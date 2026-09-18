//! The join of the three halves of a park decision.
//!
//! Three modules each answer part of one question and none of them talk:
//! - [`crate::quota::park_after`] decides WHETHER a refused run should park and
//!   until when. Its own doc says it decides about ONE ARM FROM ONE RUN and that
//!   a caller parking a whole provider is doing something it never authorised.
//! - [`crate::quota::blast_of`] and [`crate::quota::arms_in_blast`] decide HOW
//!   WIDE a refusal reaches: the arm, its vendor bucket, or the whole credential.
//! - [`crate::availability::availability`] reads a park back out and distinguishes
//!   a live park from one whose instant has passed and which nothing revisited.
//!
//! Nothing calls any of them. This module is the join. It is still pure -- it
//! writes nothing and reads no clock -- but it produces the single value a
//! dispatcher can act on.

use crate::outcome::OutcomeClass;
use crate::quota::{Blast, Park, Registry, arms_in_blast, blast_of, park_after};

/// What a dispatcher should do to the registry after one run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Change nothing. The run says nothing about availability.
    ///
    /// This variant carries no arms and no blast. A run that teaches nothing
    /// about availability must not name a blast radius: naming one invites a
    /// caller to use it.
    Leave,
    /// Park these arms until this instant. Arms sorted, deduplicated, and
    /// always containing the refused arm itself.
    ParkUntil {
        /// Epoch milliseconds.
        at_ms: u64,
        /// Every arm the refusal reaches.
        arms: Vec<String>,
        /// How wide, and why this width was chosen.
        blast: Blast,
        /// Why, carried verbatim from [`quota::Park`](crate::quota::Park). Never synthesised
        /// here: this module decides nothing about the reason. A reason
        /// this module invented would be indistinguishable from one the
        /// provider gave, and telling those apart is the entire value of
        /// the field. Carried whole without truncation: a truncated reason
        /// that still looks like a reason is worse than none.
        ///
        /// This field is a [`String`] rather than an `Option<String>`: a
        /// park always has a stated reason even when that reason is the empty
        /// string, and the absence of a park is modelled by [`Decision::Leave`],
        /// not by a null.
        grounds: String,
    },
    /// Park these arms for this long from now. Used when the provider
    /// refused without saying when it would relent: the instant is not known and must
    /// not be invented.
    ParkFor {
        /// Milliseconds from `now_ms`.
        backoff_ms: u64,
        /// Every arm the refusal reaches.
        arms: Vec<String>,
        /// How wide, and why this width was chosen.
        blast: Blast,
        /// Why, carried verbatim from [`quota::Park`](crate::quota::Park). Never synthesised
        /// here: this module decides nothing about the reason. A reason
        /// this module invented would be indistinguishable from one the
        /// provider gave, and telling those apart is the entire value of
        /// the field. Carried whole without truncation: a truncated reason
        /// that still looks like a reason is worse than none.
        ///
        /// This field is a [`String`] rather than an `Option<String>`: a
        /// park always has a stated reason even when that reason is the empty
        /// string, and the absence of a park is modelled by [`Decision::Leave`],
        /// not by a null.
        grounds: String,
    },
}

impl Decision {
    /// The justification this decision carries, if it parks anything.
    ///
    /// Delegates directly to [`grounds`].
    pub fn grounds(&self) -> Option<&str> {
        grounds(self)
    }
}

/// The justification a decision carries, if it parks anything.
///
/// `None` for [`Decision::Leave`] -- a decision that parks nothing has no
/// park to justify, and an empty string would read as "parked for no
/// stated reason", which is the thing this task exists to prevent.
///
/// Returns `Some` of exactly the stored string for either park variant,
/// including when that string is empty (`Some("")`), distinguishing an empty
/// recorded justification from the absence of a park.
pub fn grounds(d: &Decision) -> Option<&str> {
    match d {
        Decision::Leave => None,
        Decision::ParkUntil { grounds, .. } | Decision::ParkFor { grounds, .. } => {
            Some(grounds.as_str())
        }
    }
}

/// Join the three halves into one actionable decision.
///
/// `arm` is the arm that ran. `class` and `log_head` come from the finished
/// run. `registry` supplies the provider and bucket of every arm.
///
/// This function never widens beyond what [`blast_of`] returned. An
/// unrecognised refusal is [`Blast::Arm`] and the decision parks exactly one
/// arm. Widening on a guess benches arms that would have worked, and a day of
/// this project's dispatch has already been spent proving a wrong confident
/// answer costs more than an admitted narrow one.
///
/// The `grounds` string from [`quota::Park`](crate::quota::Park) is carried byte for byte into the
/// decision. This function never synthesises a reason: an empty `grounds` from
/// [`park_after`] is carried as empty. A reason this module invented would be
/// indistinguishable from one the provider gave, and telling those apart is the
/// entire value of the field. Reasons are carried whole without truncation.
pub fn decide(
    arm: &str,
    class: OutcomeClass,
    log_head: &str,
    registry: &Registry,
    now_ms: u64,
    default_backoff_ms: u64,
) -> Decision {
    let park = park_after(class, log_head, now_ms, default_backoff_ms);

    match park {
        Park::No => Decision::Leave,
        Park::Until { at_ms, grounds } => {
            let blast = blast_of(log_head);
            let arms = arms_in_blast(arm, blast, registry);
            Decision::ParkUntil {
                at_ms,
                arms,
                blast,
                grounds,
            }
        }
        Park::Backoff { until_ms, grounds } => {
            let blast = blast_of(log_head);
            let arms = arms_in_blast(arm, blast, registry);
            let backoff_ms = until_ms.saturating_sub(now_ms);
            Decision::ParkFor {
                backoff_ms,
                arms,
                blast,
                grounds,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outcome::OutcomeClass;
    use crate::quota::{Blast, Registry};

    fn registry() -> Registry {
        let mut r = Registry::new();
        r.insert("or-hy3", Some("openrouter"), Some("tencent"));
        r.insert("or-qwen38-flash", Some("openrouter"), Some("qwen"));
        r.insert("or-nemotron-ultra", Some("openrouter"), Some("nvidia"));
        r.insert("or-deepseek", Some("openrouter"), Some("deepseek"));
        r.insert("ifm-k2", Some("ifm"), Some("deepseek"));
        r.insert("ifm-k2-think", Some("ifm"), Some("deepseek"));
        r.insert("solo", Some("zai"), None);
        r
    }

    const NOW_MS: u64 = 1_789_000_000_000;
    const BACKOFF_MS: u64 = 60_000;

    // Clause 1: Any class for which park_after returns No yields Leave.
    #[test]
    fn arm_result_never_parks_even_when_log_states_reset() {
        let decision = decide(
            "or-hy3",
            OutcomeClass::ArmResult,
            "rate limit hit; retry-after: 3600",
            &registry(),
            NOW_MS,
            BACKOFF_MS,
        );
        assert_eq!(decision, Decision::Leave);
    }

    #[test]
    fn infrastructure_never_parks_even_when_log_states_reset() {
        let decision = decide(
            "or-hy3",
            OutcomeClass::Infrastructure,
            "Error: rate limit exceeded; retry-after: 3600",
            &registry(),
            NOW_MS,
            BACKOFF_MS,
        );
        assert_eq!(decision, Decision::Leave);
    }

    #[test]
    fn cancelled_never_parks() {
        let decision = decide(
            "or-hy3",
            OutcomeClass::Cancelled,
            "retry-after: 3600",
            &registry(),
            NOW_MS,
            BACKOFF_MS,
        );
        assert_eq!(decision, Decision::Leave);
    }

    #[test]
    fn unknown_never_parks() {
        let decision = decide(
            "or-hy3",
            OutcomeClass::Unknown,
            "retry-after: 3600",
            &registry(),
            NOW_MS,
            BACKOFF_MS,
        );
        assert_eq!(decision, Decision::Leave);
    }

    #[test]
    fn task_invalid_never_parks() {
        let decision = decide(
            "or-hy3",
            OutcomeClass::TaskInvalid,
            "retry-after: 3600",
            &registry(),
            NOW_MS,
            BACKOFF_MS,
        );
        assert_eq!(decision, Decision::Leave);
    }

    // Clause 2: Park::Until yields ParkUntil carrying that exact at_ms.
    #[test]
    fn stated_relative_reset_parks_until_exact_instant() {
        let log = "Error: rate limit exceeded; retry-after: 3600";
        let decision = decide(
            "or-hy3",
            OutcomeClass::QuotaLimited,
            log,
            &registry(),
            NOW_MS,
            BACKOFF_MS,
        );
        match decision {
            Decision::ParkUntil {
                at_ms, arms, blast, ..
            } => {
                assert_eq!(at_ms, 1_789_003_600_000);
                assert_eq!(blast, Blast::Arm);
                assert_eq!(arms, vec!["or-hy3"]);
            }
            other => panic!("expected ParkUntil, got {other:?}"),
        }
    }

    #[test]
    fn stated_absolute_reset_parks_until_exact_instant() {
        let log = r#"{"error": "quota", "reset_at": 1789603200}"#;
        let decision = decide(
            "or-hy3",
            OutcomeClass::QuotaLimited,
            log,
            &registry(),
            NOW_MS,
            BACKOFF_MS,
        );
        match decision {
            Decision::ParkUntil {
                at_ms, arms, blast, ..
            } => {
                assert_eq!(at_ms, 1_789_603_200_000);
                assert_eq!(blast, Blast::Arm);
                assert_eq!(arms, vec!["or-hy3"]);
            }
            other => panic!("expected ParkUntil, got {other:?}"),
        }
    }

    #[test]
    fn stated_rfc3339_reset_parks_until_exact_instant() {
        let log = "usage limit hit, resets at 2026-09-17T00:00:00Z please wait";
        let decision = decide(
            "or-hy3",
            OutcomeClass::QuotaLimited,
            log,
            &registry(),
            NOW_MS,
            BACKOFF_MS,
        );
        match decision {
            Decision::ParkUntil { at_ms, .. } => {
                assert_eq!(at_ms, 1_789_603_200_000);
            }
            other => panic!("expected ParkUntil, got {other:?}"),
        }
    }

    // Clause 3: Park::Backoff yields ParkFor carrying that exact ms.
    #[test]
    fn no_stated_reset_is_backoff_of_default_window() {
        let decision = decide(
            "or-hy3",
            OutcomeClass::QuotaLimited,
            "quota exceeded for project",
            &registry(),
            NOW_MS,
            BACKOFF_MS,
        );
        match decision {
            Decision::ParkFor {
                backoff_ms,
                arms,
                blast,
                ..
            } => {
                assert_eq!(backoff_ms, BACKOFF_MS);
                assert_eq!(blast, Blast::Arm);
                assert_eq!(arms, vec!["or-hy3"]);
            }
            other => panic!("expected ParkFor, got {other:?}"),
        }
    }

    // Clause 4: arms are exactly arms_in_blast(arm, blast_of(log_head), registry).
    #[test]
    fn credential_blast_reaches_every_arm_on_key() {
        let log = "error: key limit exceeded (total limit)";
        let decision = decide(
            "or-hy3",
            OutcomeClass::QuotaLimited,
            log,
            &registry(),
            NOW_MS,
            BACKOFF_MS,
        );
        match decision {
            Decision::ParkFor { arms, blast, .. } => {
                assert_eq!(blast, Blast::Credential);
                assert_eq!(
                    arms,
                    vec![
                        "or-deepseek",
                        "or-hy3",
                        "or-nemotron-ultra",
                        "or-qwen38-flash"
                    ]
                );
            }
            Decision::ParkUntil { arms, blast, .. } => {
                assert_eq!(blast, Blast::Credential);
                assert_eq!(
                    arms,
                    vec![
                        "or-deepseek",
                        "or-hy3",
                        "or-nemotron-ultra",
                        "or-qwen38-flash"
                    ]
                );
            }
            other => panic!("expected park, got {other:?}"),
        }
    }

    #[test]
    fn bucket_blast_reaches_arms_sharing_vendor() {
        let log = "error: token limit exceeded: tokens per day limit reached";
        let decision = decide(
            "ifm-k2",
            OutcomeClass::QuotaLimited,
            log,
            &registry(),
            NOW_MS,
            BACKOFF_MS,
        );
        match decision {
            Decision::ParkFor { arms, blast, .. } => {
                assert_eq!(blast, Blast::Bucket);
                assert_eq!(arms, vec!["ifm-k2", "ifm-k2-think", "or-deepseek"]);
            }
            Decision::ParkUntil { arms, blast, .. } => {
                assert_eq!(blast, Blast::Bucket);
                assert_eq!(arms, vec!["ifm-k2", "ifm-k2-think", "or-deepseek"]);
            }
            other => panic!("expected park, got {other:?}"),
        }
    }

    #[test]
    fn arm_blast_is_exactly_the_refused_arm() {
        let log = "error: connection reset by peer";
        let decision = decide(
            "or-hy3",
            OutcomeClass::QuotaLimited,
            log,
            &registry(),
            NOW_MS,
            BACKOFF_MS,
        );
        match decision {
            Decision::ParkFor { arms, blast, .. } => {
                assert_eq!(blast, Blast::Arm);
                assert_eq!(arms, vec!["or-hy3"]);
            }
            Decision::ParkUntil { arms, blast, .. } => {
                assert_eq!(blast, Blast::Arm);
                assert_eq!(arms, vec!["or-hy3"]);
            }
            other => panic!("expected park, got {other:?}"),
        }
    }

    // Clause 5: Leave carries no arms and no blast.
    #[test]
    fn leave_carries_no_arms_and_no_blast() {
        let decision = decide(
            "or-hy3",
            OutcomeClass::ArmResult,
            "rate limit hit; retry-after: 3600",
            &registry(),
            NOW_MS,
            BACKOFF_MS,
        );
        assert_eq!(decision, Decision::Leave);
        // The enum variant itself has no fields; this is pinned by the type.
    }

    // Clause 6: An arm absent from registry still parks itself.
    #[test]
    fn unregistered_arm_parks_itself_at_every_radius() {
        let empty_registry = Registry::new();
        for log in [
            "error: key limit exceeded (total limit)",
            "error: token limit exceeded: tokens per day limit reached",
            "error: connection reset by peer",
        ] {
            let decision = decide(
                "no-such-arm",
                OutcomeClass::QuotaLimited,
                log,
                &empty_registry,
                NOW_MS,
                BACKOFF_MS,
            );
            match decision {
                Decision::ParkFor { arms, .. } | Decision::ParkUntil { arms, .. } => {
                    assert_eq!(arms, vec!["no-such-arm"]);
                }
                Decision::Leave => panic!("unregistered arm must park, got Leave"),
            }
        }
    }

    // Clause 7: The blast reported is the one used.
    #[test]
    fn reported_blast_and_arms_membership_agree_per_variant() {
        let test_cases = [
            (
                "error: key limit exceeded (total limit)",
                Blast::Credential,
                vec![
                    "or-deepseek",
                    "or-hy3",
                    "or-nemotron-ultra",
                    "or-qwen38-flash",
                ],
            ),
            (
                "error: token limit exceeded: tokens per day limit reached",
                Blast::Bucket,
                vec!["ifm-k2", "ifm-k2-think", "or-deepseek"],
            ),
            (
                "error: connection reset by peer",
                Blast::Arm,
                vec!["or-hy3"],
            ),
        ];

        for (log, expected_blast, expected_arms) in test_cases {
            let arm = match expected_blast {
                Blast::Bucket => "ifm-k2",
                _ => "or-hy3",
            };
            let decision = decide(
                arm,
                OutcomeClass::QuotaLimited,
                log,
                &registry(),
                NOW_MS,
                BACKOFF_MS,
            );
            match decision {
                Decision::ParkFor { arms, blast, .. } | Decision::ParkUntil { arms, blast, .. } => {
                    assert_eq!(blast, expected_blast, "log: {log}");
                    assert_eq!(arms, expected_arms, "log: {log}");
                }
                Decision::Leave => panic!("expected park for QuotaLimited, log: {log}"),
            }
        }
    }

    // Clause 8: This module never widens beyond what blast_of returned.
    #[test]
    fn unrecognised_refusal_stays_narrow() {
        let decision = decide(
            "or-hy3",
            OutcomeClass::QuotaLimited,
            "error: something completely unknown",
            &registry(),
            NOW_MS,
            BACKOFF_MS,
        );
        match decision {
            Decision::ParkFor { arms, blast, .. } | Decision::ParkUntil { arms, blast, .. } => {
                assert_eq!(blast, Blast::Arm);
                assert_eq!(arms, vec!["or-hy3"]);
            }
            Decision::Leave => panic!("unrecognised refusal must still park the arm"),
        }
    }

    // Boundary: default_backoff_ms == 0 with refusal that states no reset -> Leave.
    #[test]
    fn zero_backoff_with_no_stated_reset_is_leave() {
        let decision = decide(
            "or-hy3",
            OutcomeClass::QuotaLimited,
            "quota exceeded",
            &registry(),
            NOW_MS,
            0,
        );
        assert_eq!(decision, Decision::Leave);
    }

    // Boundary: reset instant exactly equal to now_ms -> Leave.
    #[test]
    fn reset_exactly_at_now_is_leave() {
        let log = r#"{"error": "quota", "reset_at": 1789000000}"#; // 1789000000 * 1000 = NOW_MS
        let decision = decide(
            "or-hy3",
            OutcomeClass::QuotaLimited,
            log,
            &registry(),
            NOW_MS,
            BACKOFF_MS,
        );
        assert_eq!(decision, Decision::Leave);
    }

    // Boundary: empty registry - park still names refused arm.
    #[test]
    fn empty_registry_parks_refused_arm_only() {
        let empty = Registry::new();
        let decision = decide(
            "or-hy3",
            OutcomeClass::QuotaLimited,
            "quota exceeded",
            &empty,
            NOW_MS,
            BACKOFF_MS,
        );
        match decision {
            Decision::ParkFor { arms, .. } | Decision::ParkUntil { arms, .. } => {
                assert_eq!(arms, vec!["or-hy3"]);
            }
            Decision::Leave => panic!("empty registry must still park the refused arm"),
        }
    }

    // Boundary: empty log_head with QuotaLimited -> ParkFor over one arm.
    #[test]
    fn empty_log_head_with_quota_limited_is_parkfor_over_one_arm() {
        let decision = decide(
            "or-hy3",
            OutcomeClass::QuotaLimited,
            "",
            &registry(),
            1_000,
            1,
        );
        match decision {
            Decision::ParkFor {
                backoff_ms,
                arms,
                blast,
                ..
            } => {
                assert_eq!(backoff_ms, 1);
                assert_eq!(arms, vec!["or-hy3"]);
                assert_eq!(blast, Blast::Arm);
            }
            other => panic!("expected ParkFor, got {other:?}"),
        }
    }

    // Boundary: now_ms == 0 - no arithmetic underflows.
    #[test]
    fn zero_now_ms_handled_cleanly() {
        let decision = decide(
            "or-hy3",
            OutcomeClass::QuotaLimited,
            r#""reset_at": 500"#,
            &registry(),
            0,
            0,
        );
        match decision {
            Decision::ParkUntil {
                at_ms, arms, blast, ..
            } => {
                assert_eq!(at_ms, 500_000);
                assert_eq!(arms, vec!["or-hy3"]);
                assert_eq!(blast, Blast::Arm);
            }
            other => panic!("expected ParkUntil, got {other:?}"),
        }
    }

    // Composition: arms is sorted, deduplicated, never empty in park variants.
    #[test]
    fn arms_are_sorted_deduplicated_and_non_empty() {
        let mut r = Registry::new();
        // Insert arms with same provider/bucket to test dedup
        r.insert("a", Some("p"), Some("b"));
        r.insert("b", Some("p"), Some("b"));
        r.insert("c", Some("p"), Some("b"));

        let decision = decide(
            "a",
            OutcomeClass::QuotaLimited,
            "error: key limit exceeded (total limit)",
            &r,
            NOW_MS,
            BACKOFF_MS,
        );
        match decision {
            Decision::ParkFor { arms, .. } | Decision::ParkUntil { arms, .. } => {
                assert!(!arms.is_empty());
                // Check sorted
                let mut sorted = arms.clone();
                sorted.sort();
                assert_eq!(arms, sorted);
                // Check no duplicates
                let mut deduped = arms.clone();
                deduped.dedup();
                assert_eq!(arms, deduped);
            }
            Decision::Leave => panic!("expected park"),
        }
    }

    // --- Tests for grounds field, grounds() reader, and falsifiable clauses ---

    // Clause 1: ParkUntil carries grounds from Park::Until byte for byte.
    #[test]
    fn clause1_park_until_carries_grounds_byte_for_byte_relative_reset() {
        let log = "Error: rate limit exceeded; retry-after: 3600";
        let park = park_after(OutcomeClass::QuotaLimited, log, NOW_MS, BACKOFF_MS);
        let expected_grounds = match park {
            Park::Until { grounds, .. } => grounds,
            other => panic!("expected Park::Until from park_after, got {other:?}"),
        };

        let decision = decide(
            "or-hy3",
            OutcomeClass::QuotaLimited,
            log,
            &registry(),
            NOW_MS,
            BACKOFF_MS,
        );

        match &decision {
            Decision::ParkUntil { grounds: g, .. } => {
                assert_eq!(g, &expected_grounds);
                assert_eq!(g.as_bytes(), expected_grounds.as_bytes());
            }
            other => panic!("expected Decision::ParkUntil, got {other:?}"),
        }

        assert_eq!(grounds(&decision), Some(expected_grounds.as_str()));
        assert_eq!(decision.grounds(), Some(expected_grounds.as_str()));

        // Confirm not trimmed, not lowercased, not truncated
        assert!(expected_grounds.chars().any(|c| c.is_uppercase()));
        assert_eq!(grounds(&decision).unwrap().len(), expected_grounds.len());
    }

    #[test]
    fn clause1_park_until_carries_grounds_byte_for_byte_rfc3339_reset() {
        let log = "usage limit hit, resets at 2026-09-17T00:00:00Z please wait";
        let park = park_after(OutcomeClass::QuotaLimited, log, NOW_MS, BACKOFF_MS);
        let expected_grounds = match park {
            Park::Until { grounds, .. } => grounds,
            other => panic!("expected Park::Until from park_after, got {other:?}"),
        };

        let decision = decide(
            "or-hy3",
            OutcomeClass::QuotaLimited,
            log,
            &registry(),
            NOW_MS,
            BACKOFF_MS,
        );

        match &decision {
            Decision::ParkUntil { grounds: g, .. } => {
                assert_eq!(g, &expected_grounds);
                assert_eq!(g.as_bytes(), expected_grounds.as_bytes());
            }
            other => panic!("expected Decision::ParkUntil, got {other:?}"),
        }

        assert_eq!(grounds(&decision), Some(expected_grounds.as_str()));
    }

    #[test]
    fn clause1_park_until_carries_grounds_byte_for_byte_json_reset() {
        let log = r#"{"error": "quota", "reset_at": 1789603200}"#;
        let park = park_after(OutcomeClass::QuotaLimited, log, NOW_MS, BACKOFF_MS);
        let expected_grounds = match park {
            Park::Until { grounds, .. } => grounds,
            other => panic!("expected Park::Until from park_after, got {other:?}"),
        };

        let decision = decide(
            "or-hy3",
            OutcomeClass::QuotaLimited,
            log,
            &registry(),
            NOW_MS,
            BACKOFF_MS,
        );

        match &decision {
            Decision::ParkUntil { grounds: g, .. } => {
                assert_eq!(g, &expected_grounds);
                assert_eq!(g.as_bytes(), expected_grounds.as_bytes());
            }
            other => panic!("expected Decision::ParkUntil, got {other:?}"),
        }

        assert_eq!(grounds(&decision), Some(expected_grounds.as_str()));
    }

    // Clause 2: ParkFor carries grounds from Park::Backoff byte for byte.
    #[test]
    fn clause2_park_for_carries_grounds_byte_for_byte_default_window() {
        let log = "quota exceeded for project";
        let park = park_after(OutcomeClass::QuotaLimited, log, NOW_MS, BACKOFF_MS);
        let expected_grounds = match park {
            Park::Backoff { grounds, .. } => grounds,
            other => panic!("expected Park::Backoff from park_after, got {other:?}"),
        };

        let decision = decide(
            "or-hy3",
            OutcomeClass::QuotaLimited,
            log,
            &registry(),
            NOW_MS,
            BACKOFF_MS,
        );

        match &decision {
            Decision::ParkFor { grounds: g, .. } => {
                assert_eq!(g, &expected_grounds);
                assert_eq!(g.as_bytes(), expected_grounds.as_bytes());
            }
            other => panic!("expected Decision::ParkFor, got {other:?}"),
        }

        assert_eq!(grounds(&decision), Some(expected_grounds.as_str()));
        assert_eq!(decision.grounds(), Some(expected_grounds.as_str()));
    }

    #[test]
    fn clause2_park_for_carries_grounds_byte_for_byte_empty_log() {
        let log = "";
        let park = park_after(OutcomeClass::QuotaLimited, log, NOW_MS, BACKOFF_MS);
        let expected_grounds = match park {
            Park::Backoff { grounds, .. } => grounds,
            other => panic!("expected Park::Backoff from park_after, got {other:?}"),
        };

        let decision = decide(
            "or-hy3",
            OutcomeClass::QuotaLimited,
            log,
            &registry(),
            NOW_MS,
            BACKOFF_MS,
        );

        match &decision {
            Decision::ParkFor { grounds: g, .. } => {
                assert_eq!(g, &expected_grounds);
                assert_eq!(g.as_bytes(), expected_grounds.as_bytes());
            }
            other => panic!("expected Decision::ParkFor, got {other:?}"),
        }

        assert_eq!(grounds(&decision), Some(expected_grounds.as_str()));
    }

    // Clause 3: decide never synthesises a grounds string.
    #[test]
    fn clause3_decide_never_synthesises_grounds() {
        // decide carries grounds from park_after verbatim without inventing its own
        let log = "quota exceeded";
        let park = park_after(OutcomeClass::QuotaLimited, log, NOW_MS, BACKOFF_MS);
        let park_grounds = match park {
            Park::Backoff { grounds, .. } => grounds,
            _ => panic!(),
        };

        let decision = decide(
            "or-hy3",
            OutcomeClass::QuotaLimited,
            log,
            &registry(),
            NOW_MS,
            BACKOFF_MS,
        );
        let dec_grounds = grounds(&decision).expect("must have grounds");
        assert_eq!(dec_grounds, park_grounds.as_str());

        // An empty grounds string in Decision is carried verbatim and not replaced
        let empty_until = Decision::ParkUntil {
            at_ms: NOW_MS + 10_000,
            arms: vec!["or-hy3".into()],
            blast: Blast::Arm,
            grounds: String::new(),
        };
        assert_eq!(grounds(&empty_until), Some(""));

        let empty_for = Decision::ParkFor {
            backoff_ms: 10_000,
            arms: vec!["or-hy3".into()],
            blast: Blast::Arm,
            grounds: String::new(),
        };
        assert_eq!(grounds(&empty_for), Some(""));
    }

    // Clause 4: grounds(&Decision::Leave) is None. Pin None != Some("").
    #[test]
    fn clause4_grounds_leave_is_none_and_not_some_empty() {
        assert_eq!(grounds(&Decision::Leave), None);
        assert_ne!(grounds(&Decision::Leave), Some(""));
        assert!(grounds(&Decision::Leave).is_none());
        assert_eq!(Decision::Leave.grounds(), None);

        // Every class returning Leave has grounds() == None
        for class in [
            OutcomeClass::ArmResult,
            OutcomeClass::Infrastructure,
            OutcomeClass::Cancelled,
            OutcomeClass::Unknown,
            OutcomeClass::TaskInvalid,
        ] {
            let decision = decide(
                "or-hy3",
                class,
                "Error: rate limit exceeded; retry-after: 3600",
                &registry(),
                NOW_MS,
                BACKOFF_MS,
            );
            assert_eq!(decision, Decision::Leave);
            assert_eq!(grounds(&decision), None);
            assert_ne!(grounds(&decision), Some(""));
        }

        // Closed/expired window yields Leave with grounds() == None
        let past_reset = decide(
            "or-hy3",
            OutcomeClass::QuotaLimited,
            r#"{"error": "quota", "reset_at": 100}"#,
            &registry(),
            NOW_MS,
            BACKOFF_MS,
        );
        assert_eq!(past_reset, Decision::Leave);
        assert_eq!(grounds(&past_reset), None);

        // Reset at now_ms yields Leave with grounds() == None
        let now_reset = decide(
            "or-hy3",
            OutcomeClass::QuotaLimited,
            r#"{"error": "quota", "reset_at": 1789000000}"#,
            &registry(),
            NOW_MS,
            BACKOFF_MS,
        );
        assert_eq!(now_reset, Decision::Leave);
        assert_eq!(grounds(&now_reset), None);

        // Zero backoff with no stated reset yields Leave with grounds() == None
        let zero_backoff = decide(
            "or-hy3",
            OutcomeClass::QuotaLimited,
            "quota exceeded",
            &registry(),
            NOW_MS,
            0,
        );
        assert_eq!(zero_backoff, Decision::Leave);
        assert_eq!(grounds(&zero_backoff), None);
    }

    // Clause 5: grounds on either park variant returns Some of exactly the stored string,
    // including when that string is empty (Some("") is real and distinct from None).
    #[test]
    fn clause5_grounds_reader_on_park_variants() {
        let stored = "provider stated a reset";
        let d_until = Decision::ParkUntil {
            at_ms: NOW_MS + 10_000,
            arms: vec!["or-hy3".into()],
            blast: Blast::Arm,
            grounds: stored.to_string(),
        };
        assert_eq!(grounds(&d_until), Some(stored));
        assert_eq!(d_until.grounds(), Some(stored));

        let d_for = Decision::ParkFor {
            backoff_ms: 10_000,
            arms: vec!["or-hy3".into()],
            blast: Blast::Arm,
            grounds: stored.to_string(),
        };
        assert_eq!(grounds(&d_for), Some(stored));
        assert_eq!(d_for.grounds(), Some(stored));

        // When stored string is empty:
        let d_until_empty = Decision::ParkUntil {
            at_ms: NOW_MS + 10_000,
            arms: vec!["or-hy3".into()],
            blast: Blast::Arm,
            grounds: String::new(),
        };
        assert_eq!(grounds(&d_until_empty), Some(""));
        assert!(grounds(&d_until_empty).is_some());
        assert_ne!(grounds(&d_until_empty), None);

        let d_for_empty = Decision::ParkFor {
            backoff_ms: 10_000,
            arms: vec!["or-hy3".into()],
            blast: Blast::Arm,
            grounds: String::new(),
        };
        assert_eq!(grounds(&d_for_empty), Some(""));
        assert!(grounds(&d_for_empty).is_some());
        assert_ne!(grounds(&d_for_empty), None);
    }

    // Boundary: An empty grounds from park_after carried as empty, grounds() returns Some("").
    #[test]
    fn boundary_empty_grounds_returns_some_empty_string() {
        let d1 = Decision::ParkUntil {
            at_ms: 12345,
            arms: vec!["arm".into()],
            blast: Blast::Arm,
            grounds: "".into(),
        };
        assert_eq!(grounds(&d1), Some(""));
        assert_ne!(grounds(&d1), None);

        let d2 = Decision::ParkFor {
            backoff_ms: 54321,
            arms: vec!["arm".into()],
            blast: Blast::Arm,
            grounds: "".into(),
        };
        assert_eq!(grounds(&d2), Some(""));
        assert_ne!(grounds(&d2), None);
    }

    // Boundary: A grounds containing newlines, tabs, escapes, special chars, nulls.
    #[test]
    fn boundary_grounds_with_newlines_tabs_and_escapes_carried_verbatim() {
        let complex =
            "Provider Error:\nLine 2\r\n\tTabbed\0with \\escapes\\ and symbols: !@#$%^&*()";
        let d_until = Decision::ParkUntil {
            at_ms: NOW_MS + 5_000,
            arms: vec!["or-hy3".into()],
            blast: Blast::Arm,
            grounds: complex.to_string(),
        };
        assert_eq!(grounds(&d_until), Some(complex));
        assert_eq!(grounds(&d_until).unwrap().as_bytes(), complex.as_bytes());

        let d_for = Decision::ParkFor {
            backoff_ms: 5_000,
            arms: vec!["or-hy3".into()],
            blast: Blast::Arm,
            grounds: complex.to_string(),
        };
        assert_eq!(grounds(&d_for), Some(complex));
        assert_eq!(grounds(&d_for).unwrap().as_bytes(), complex.as_bytes());
    }

    // Boundary: A very long grounds carried whole without truncation.
    #[test]
    fn boundary_very_long_grounds_carried_whole_no_truncation() {
        let long_reason = "Detailed upstream refusal explanation: ".repeat(5_000);
        assert!(long_reason.len() > 100_000);

        let d_until = Decision::ParkUntil {
            at_ms: NOW_MS + 5_000,
            arms: vec!["or-hy3".into()],
            blast: Blast::Arm,
            grounds: long_reason.clone(),
        };
        assert_eq!(grounds(&d_until), Some(long_reason.as_str()));
        assert_eq!(grounds(&d_until).unwrap().len(), long_reason.len());

        let d_for = Decision::ParkFor {
            backoff_ms: 5_000,
            arms: vec!["or-hy3".into()],
            blast: Blast::Arm,
            grounds: long_reason.clone(),
        };
        assert_eq!(grounds(&d_for), Some(long_reason.as_str()));
        assert_eq!(grounds(&d_for).unwrap().len(), long_reason.len());
    }

    // Boundary: Whitespace preserved, not trimmed.
    #[test]
    fn boundary_whitespace_preserved_not_trimmed() {
        let padded = "   \t  lots of whitespace \n  \r\n  ";
        let d = Decision::ParkUntil {
            at_ms: 1000,
            arms: vec!["arm".into()],
            blast: Blast::Arm,
            grounds: padded.to_string(),
        };
        assert_eq!(grounds(&d), Some(padded));
        assert_eq!(grounds(&d).unwrap(), padded);
    }

    // Boundary: Case preserved, not lowercased.
    #[test]
    fn boundary_case_preserved_not_lowercased() {
        let uppercase = "PROVIDER STATED RESET IN 3600 SECONDS";
        let d = Decision::ParkFor {
            backoff_ms: 1000,
            arms: vec!["arm".into()],
            blast: Blast::Arm,
            grounds: uppercase.to_string(),
        };
        assert_eq!(grounds(&d), Some(uppercase));
        assert_eq!(grounds(&d).unwrap(), uppercase);
    }

    // Boundary: Decision::Leave has no grounds field at all; it remains a unit variant.
    #[test]
    fn boundary_decision_leave_is_unit_variant() {
        let d = Decision::Leave;
        match d {
            Decision::Leave => {}
            Decision::ParkUntil { .. } | Decision::ParkFor { .. } => {
                panic!("expected unit variant Leave")
            }
        }
    }

    // Boundary: Decision is a closed set of three variants.
    #[test]
    fn boundary_decision_is_closed_three_variants() {
        let variants = [
            Decision::Leave,
            Decision::ParkUntil {
                at_ms: 1,
                arms: vec!["a".into()],
                blast: Blast::Arm,
                grounds: "r".into(),
            },
            Decision::ParkFor {
                backoff_ms: 1,
                arms: vec!["a".into()],
                blast: Blast::Arm,
                grounds: "r".into(),
            },
        ];

        for v in variants {
            match v {
                Decision::Leave => {}
                Decision::ParkUntil { .. } => {}
                Decision::ParkFor { .. } => {}
            }
        }
    }
}
