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
    },
    /// Park these arms for this long from now. Used when the provider
    /// refused without stating a reset: the instant is not known and must
    /// not be invented.
    ParkFor {
        /// Milliseconds from `now_ms`.
        backoff_ms: u64,
        /// Every arm the refusal reaches.
        arms: Vec<String>,
        /// How wide, and why this width was chosen.
        blast: Blast,
    },
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
        Park::Until { at_ms, .. } => {
            let blast = blast_of(log_head);
            let arms = arms_in_blast(arm, blast, registry);
            Decision::ParkUntil { at_ms, arms, blast }
        }
        Park::Backoff { until_ms, .. } => {
            let blast = blast_of(log_head);
            let arms = arms_in_blast(arm, blast, registry);
            let backoff_ms = until_ms.saturating_sub(now_ms);
            Decision::ParkFor {
                backoff_ms,
                arms,
                blast,
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
            Decision::ParkUntil { at_ms, arms, blast } => {
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
            Decision::ParkUntil { at_ms, arms, blast } => {
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
            Decision::ParkUntil { at_ms, arms, blast } => {
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
}
