//! Implementer router: which agent attempts a task, and why the others do not.
//!
//! Two things make picking an implementer harder than argmax over a success
//! rate.
//!
//! **Eligibility is not capability.** An arm whose provider is rate-limited, or
//! which cannot do multi-turn work, or which is a paid route to a model already
//! available free, must not be selected — and none of that is evidence about how
//! good it is. A rate limit says nothing about whether an arm would have solved
//! the task; folding it into the posterior would let a transient outage
//! permanently demote a good arm. So [`eligibility`] is a pre-dispatch *filter*
//! that returns a reason, and the posterior is never touched by it.
//!
//! **Exploration must be cheap, not free.** An arm with no history might be
//! excellent. Thompson sampling handles that: draw a success probability from
//! each arm's posterior and take the best draw, so uncertain arms win sometimes
//! and consistently-good arms win usually. An arm whose posterior is tight and
//! high wins often; an arm about which nothing is known keeps getting tried.
//!
//! The draws themselves are supplied by the caller ([`select`]'s `draws`), which
//! is what keeps this module pure: no I/O, no clock, and no randomness of its
//! own. Given the same roster and the same draws, [`select`] returns the same
//! [`Choice`].

use serde::{Deserialize, Serialize};

use crate::attempt_log::Posterior;

/// Identity of an implementer arm, e.g. `ArmName("claude-sonnet".to_string())`.
///
/// Opaque so that a name cannot be confused with a capability, a bucket, or a
/// free-text reason.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArmName(pub String);

impl ArmName {
    /// Borrow the underlying name.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// Why an arm may not be dispatched right now.
///
/// Every variant is a statement about the *situation*, never about the arm's
/// ability: none of them is evidence, so none of them moves a posterior.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Ineligible {
    /// The arm's provider asked us to back off; it may be used again at `until`.
    Parked {
        /// Epoch second at which the arm becomes selectable again.
        until: u64,
    },
    /// The task requires a capability this arm does not advertise.
    MissingCapability {
        /// The required capability the arm lacks.
        needed: String,
    },
    /// A free arm already covers this model, so paying for it is waste.
    RedundantPaidRoute {
        /// The healthy free arm that makes this one redundant.
        free_arm: String,
    },
    /// An operator turned the arm off.
    Disabled {
        /// Why the arm was disabled.
        reason: String,
    },
}

/// Everything the router knows about one arm at dispatch time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArmInfo {
    /// Identity of the arm.
    pub name: ArmName,
    /// Success posterior over this arm's own attempts.
    pub posterior: Posterior,
    /// Dollars per 1M input tokens; zero for plan-based arms.
    pub price_in: f64,
    /// Observed mean latency in seconds. `None` when never measured.
    pub mean_latency_s: Option<f64>,
    /// Capabilities this arm has, e.g. `"multiturn"`.
    pub capabilities: Vec<String>,
    /// Epoch second until which the arm is parked, if it is parked.
    pub parked_until: Option<u64>,
    /// A free arm that already serves this model, if there is one.
    pub redundant_with: Option<String>,
    /// Why an operator disabled the arm, if it is disabled.
    pub disabled_reason: Option<String>,
}

/// How much each axis of a choice is worth.
///
/// All three are in the same units as `score`'s result, so a weight is directly
/// a price: `dollar_weight` is how many units of task value one dollar per 1M
/// input tokens costs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Weights {
    /// Value of succeeding at the task at all.
    pub task_value: f64,
    /// Cost of one dollar per 1M input tokens.
    pub dollar_weight: f64,
    /// Cost of one second of latency.
    pub latency_weight: f64,
}

/// The arm [`select`] chose, and the draw that won it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    /// The selected arm.
    pub arm: ArmName,
    /// The winning arm's score.
    pub score: f64,
    /// The success probability sampled for the winning arm.
    pub sampled_p: f64,
}

/// Why an arm was excluded, or `None` when it may be dispatched.
///
/// Eligibility is a filter, never a score: an arm is either dispatchable or it
/// is not, and a verdict here says nothing about how good the arm is. When
/// several reasons apply at once the first is reported, in the order the
/// variants of [`Ineligible`] are declared.
pub fn eligibility(
    arm: &ArmInfo,
    needed: &[String],
    now: u64,
    healthy_free_arms: &[String],
) -> Option<Ineligible> {
    if let Some(until) = arm.parked_until
        && now < until
    {
        return Some(Ineligible::Parked { until });
    }

    for need in needed {
        if !arm.capabilities.iter().any(|has| has == need) {
            return Some(Ineligible::MissingCapability {
                needed: need.clone(),
            });
        }
    }

    if let Some(free_arm) = &arm.redundant_with
        && healthy_free_arms.iter().any(|h| h == free_arm)
    {
        return Some(Ineligible::RedundantPaidRoute {
            free_arm: free_arm.clone(),
        });
    }

    if let Some(reason) = &arm.disabled_reason {
        return Some(Ineligible::Disabled {
            reason: reason.clone(),
        });
    }

    None
}

/// Score one arm given an already-drawn success probability.
///
/// `score = task_value * p - dollar_weight * price_in - latency_weight * latency`.
///
/// An unmeasured latency (`mean_latency_s == None`) is treated as the worst
/// latency observed anywhere in `all`, never as zero: the absence of data is not
/// a good result, and scoring it as zero would make a never-measured arm look
/// faster than every arm that has been timed. When nothing has been measured
/// anywhere, the latency term is dropped rather than invented.
///
/// The result is always finite: non-finite inputs are treated as zero, so no NaN
/// escapes into a comparison.
pub fn score(arm: &ArmInfo, sampled_p: f64, w: &Weights, all: &[ArmInfo]) -> f64 {
    let latency = arm
        .mean_latency_s
        .filter(|l| l.is_finite())
        .or_else(|| worst_observed_latency(all))
        .unwrap_or(0.0);

    let value = finite(w.task_value) * finite(sampled_p)
        - finite(w.dollar_weight) * finite(arm.price_in)
        - finite(w.latency_weight) * latency;

    if value.is_finite() { value } else { 0.0 }
}

/// Pick an arm by Thompson sampling over the eligible arms.
///
/// `draws` supplies one already-drawn success probability per eligible arm, in
/// the order those arms appear in `arms` after filtering. Arms that are
/// ineligible are skipped and consume no draw. Only as many arms are scored as
/// there are draws, so a short `draws` slice narrows the field instead of
/// panicking.
///
/// Returns `None` when no arm is eligible, or when `draws` is empty. Ties go to
/// the arm earliest in `arms`.
pub fn select(
    arms: &[ArmInfo],
    needed: &[String],
    now: u64,
    healthy_free_arms: &[String],
    w: &Weights,
    draws: &[f64],
) -> Option<Choice> {
    let mut best: Option<Choice> = None;
    let mut next_draw = 0usize;

    for arm in arms {
        if eligibility(arm, needed, now, healthy_free_arms).is_some() {
            continue;
        }

        let Some(&sampled_p) = draws.get(next_draw) else {
            break;
        };
        next_draw += 1;

        let arm_score = score(arm, sampled_p, w, arms);
        if best.as_ref().is_none_or(|b| arm_score > b.score) {
            best = Some(Choice {
                arm: arm.name.clone(),
                score: arm_score,
                sampled_p,
            });
        }
    }

    best
}

/// The slowest latency measured anywhere in `all`, if any was measured.
fn worst_observed_latency(all: &[ArmInfo]) -> Option<f64> {
    all.iter()
        .filter_map(|a| a.mean_latency_s)
        .filter(|l| l.is_finite())
        .reduce(f64::max)
}

/// A non-finite number contributes nothing rather than poisoning the result.
fn finite(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_000;

    fn arm(name: &str) -> ArmInfo {
        ArmInfo {
            name: ArmName(name.to_string()),
            posterior: Posterior {
                alpha: 1.0,
                beta: 1.0,
            },
            price_in: 0.0,
            mean_latency_s: None,
            capabilities: Vec::new(),
            parked_until: None,
            redundant_with: None,
            disabled_reason: None,
        }
    }

    fn weights() -> Weights {
        Weights {
            task_value: 1.0,
            dollar_weight: 0.5,
            latency_weight: 0.5,
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn a_plain_healthy_arm_is_eligible() {
        let a = arm("a");
        assert_eq!(eligibility(&a, &[], NOW, &[]), None);
    }

    #[test]
    fn parked_arm_is_skipped_and_its_posterior_is_unchanged() {
        let mut parked = arm("parked");
        parked.posterior = Posterior {
            alpha: 9.0,
            beta: 2.0,
        };
        parked.parked_until = Some(NOW + 60);
        let other = arm("other");

        let arms = vec![parked.clone(), other];
        let choice = select(&arms, &[], NOW, &[], &weights(), &[0.99, 0.01]);

        assert_eq!(choice.map(|c| c.arm), Some(ArmName("other".to_string())));
        assert_eq!(arms[0].posterior, parked.posterior);
        assert_eq!(arms[0].parked_until, Some(NOW + 60));

        // The better posterior lost, because eligibility is a filter: it ranks
        // nothing, it only removes.
        assert!(arms[0].posterior.mean() > arms[1].posterior.mean());
    }

    #[test]
    fn parked_arm_is_ineligible_only_strictly_before_its_until() {
        let mut parked = arm("parked");
        parked.parked_until = Some(NOW + 1);

        assert!(matches!(
            eligibility(&parked, &[], NOW, &[]),
            Some(Ineligible::Parked { until }) if until == NOW + 1
        ));
        assert_eq!(eligibility(&parked, &[], NOW + 1, &[]), None);
        assert_eq!(eligibility(&parked, &[], NOW + 2, &[]), None);
    }

    #[test]
    fn paid_duplicate_is_skipped_while_its_free_twin_is_healthy() {
        let free = arm("free");
        let mut paid = arm("paid");
        paid.price_in = 3.0;
        paid.redundant_with = Some("free".to_string());

        let arms = vec![paid.clone(), free.clone()];
        let healthy = vec!["free".to_string()];

        assert!(matches!(
            eligibility(&paid, &[], NOW, &healthy),
            Some(Ineligible::RedundantPaidRoute { .. })
        ));
        let choice = select(&arms, &[], NOW, &healthy, &weights(), &[0.99, 0.01]);
        assert_eq!(choice.map(|c| c.arm), Some(ArmName("free".to_string())));
    }

    #[test]
    fn paid_duplicate_is_selectable_once_its_free_twin_is_not_healthy() {
        let mut paid = arm("paid");
        paid.price_in = 3.0;
        paid.redundant_with = Some("free".to_string());
        let other = arm("other");

        let arms = vec![paid.clone(), other];
        let w = Weights {
            task_value: 1.0,
            dollar_weight: 0.0,
            latency_weight: 0.0,
        };

        assert_eq!(eligibility(&paid, &[], NOW, &[]), None);
        let choice = select(&arms, &[], NOW, &[], &w, &[0.99, 0.01]);
        assert_eq!(choice.map(|c| c.arm), Some(ArmName("paid".to_string())));
    }

    #[test]
    fn arm_missing_a_needed_capability_is_ineligible() {
        let mut a = arm("a");
        a.capabilities = vec!["singleturn".to_string()];

        let needed = vec!["multiturn".to_string()];
        assert!(matches!(
            eligibility(&a, &needed, NOW, &[]),
            Some(Ineligible::MissingCapability { .. })
        ));

        a.capabilities.push("multiturn".to_string());
        assert_eq!(eligibility(&a, &needed, NOW, &[]), None);
    }

    #[test]
    fn a_reported_missing_capability_is_one_the_arm_really_lacks() {
        let mut a = arm("a");
        a.capabilities = vec!["singleturn".to_string()];
        let required = vec!["multiturn".to_string(), "tooluse".to_string()];

        let verdict = eligibility(&a, &required, NOW, &[]);
        assert!(
            matches!(&verdict, Some(Ineligible::MissingCapability { needed })
                if required.contains(needed) && !a.capabilities.contains(needed)),
            "expected a capability the arm lacks, got {verdict:?}"
        );
    }

    #[test]
    fn needing_no_capabilities_filters_nothing_out() {
        let a = arm("a");
        assert_eq!(eligibility(&a, &[], NOW, &[]), None);
        assert!(select(&[a], &[], NOW, &[], &weights(), &[0.5]).is_some());
    }

    #[test]
    fn disabled_arm_is_ineligible() {
        let mut a = arm("a");
        a.disabled_reason = Some("provider sunset".to_string());

        assert!(matches!(
            eligibility(&a, &[], NOW, &[]),
            Some(Ineligible::Disabled { .. })
        ));
        assert_eq!(select(&[a], &[], NOW, &[], &weights(), &[0.9]), None);
    }

    #[test]
    fn unmeasured_latency_is_scored_as_the_worst_observed_latency() {
        let mut fast = arm("fast");
        fast.mean_latency_s = Some(2.0);
        let mut slow = arm("slow");
        slow.mean_latency_s = Some(8.0);
        let unmeasured = arm("unmeasured");

        let all = vec![fast.clone(), slow.clone(), unmeasured.clone()];
        let w = weights();

        assert!(close(
            score(&unmeasured, 0.5, &w, &all),
            score(&slow, 0.5, &w, &all)
        ));
        assert!(score(&unmeasured, 0.5, &w, &all) < score(&fast, 0.5, &w, &all));
    }

    #[test]
    fn unmeasured_latency_is_never_scored_as_zero() {
        let mut fast = arm("fast");
        fast.mean_latency_s = Some(2.0);
        let unmeasured = arm("unmeasured");

        let all = vec![fast.clone(), unmeasured.clone()];
        let w = Weights {
            task_value: 1.0,
            dollar_weight: 0.0,
            latency_weight: 0.5,
        };

        // With no latency penalty the arm would score task_value * p.
        let flattered = w.task_value * 0.5;
        assert!(score(&unmeasured, 0.5, &w, &all) < flattered);
    }

    #[test]
    fn score_is_finite_when_nothing_was_measured_and_weights_are_zero() {
        let unmeasured = arm("unmeasured");
        let w = Weights {
            task_value: 0.0,
            dollar_weight: 0.0,
            latency_weight: 0.0,
        };

        let s = score(&unmeasured, 0.5, &w, &[]);
        assert!(s.is_finite());
        assert!(close(s, 0.0));

        let measured = vec![unmeasured.clone()];
        assert!(close(score(&unmeasured, f64::NAN, &w, &measured), 0.0));
        assert!(score(&unmeasured, 0.5, &weights(), &[]).is_finite());
    }

    #[test]
    fn a_higher_sampled_probability_scores_higher() {
        let a = arm("a");
        let all = vec![a.clone()];
        assert!(score(&a, 0.9, &weights(), &all) > score(&a, 0.1, &weights(), &all));
    }

    #[test]
    fn a_higher_price_scores_lower() {
        let cheap = arm("cheap");
        let mut dear = arm("dear");
        dear.price_in = 4.0;

        let all = vec![cheap.clone(), dear.clone()];
        let w = weights();
        assert!(score(&dear, 0.5, &w, &all) < score(&cheap, 0.5, &w, &all));
    }

    #[test]
    fn a_higher_latency_scores_lower() {
        let mut quick = arm("quick");
        quick.mean_latency_s = Some(1.0);
        let mut slow = arm("slow");
        slow.mean_latency_s = Some(9.0);

        let all = vec![quick.clone(), slow.clone()];
        let w = weights();
        assert!(score(&slow, 0.5, &w, &all) < score(&quick, 0.5, &w, &all));
    }

    #[test]
    fn identical_draws_give_identical_selections() {
        let mut a = arm("a");
        a.posterior = Posterior {
            alpha: 4.0,
            beta: 2.0,
        };
        let b = arm("b");
        let arms = vec![a, b];
        let draws = [0.31, 0.87];

        let first = select(&arms, &[], NOW, &[], &weights(), &draws);
        let second = select(&arms, &[], NOW, &[], &weights(), &draws);
        assert_eq!(first, second);

        let rebuilt = arms.clone();
        let third = select(&rebuilt, &[], NOW, &[], &weights(), &draws);
        assert_eq!(first, third);
    }

    #[test]
    fn select_with_an_empty_roster_returns_none() {
        assert_eq!(select(&[], &[], NOW, &[], &weights(), &[]), None);
        assert_eq!(select(&[], &[], NOW, &[], &weights(), &[0.5, 0.6]), None);
    }

    #[test]
    fn select_returns_none_when_no_arm_is_eligible() {
        let mut a = arm("a");
        a.disabled_reason = Some("off".to_string());
        let mut b = arm("b");
        b.parked_until = Some(NOW + 5);

        assert_eq!(
            select(&[a, b], &[], NOW, &[], &weights(), &[0.9, 0.9]),
            None
        );
    }

    #[test]
    fn select_returns_none_when_there_are_no_draws() {
        let arms = vec![arm("a"), arm("b")];
        assert_eq!(select(&arms, &[], NOW, &[], &weights(), &[]), None);
    }

    #[test]
    fn only_as_many_arms_are_scored_as_there_are_draws() {
        let arms = vec![arm("a"), arm("b")];

        // One draw: only the first eligible arm is scored, so it wins even
        // though the second would have won with the higher draw.
        let one = select(&arms, &[], NOW, &[], &weights(), &[0.1]);
        assert_eq!(
            one.as_ref().map(|c| c.arm.clone()),
            Some(ArmName("a".to_string()))
        );

        let two = select(&arms, &[], NOW, &[], &weights(), &[0.1, 0.9]);
        assert_eq!(
            two.as_ref().map(|c| c.arm.clone()),
            Some(ArmName("b".to_string()))
        );
    }

    #[test]
    fn ineligible_arms_consume_no_draw() {
        let mut parked = arm("parked");
        parked.parked_until = Some(NOW + 30);
        let a = arm("a");
        let b = arm("b");
        let arms = vec![parked, a, b];

        // Draws go to the eligible arms in roster order: a gets 0.1, b gets 0.9.
        let choice = select(&arms, &[], NOW, &[], &weights(), &[0.1, 0.9]);
        assert_eq!(
            choice.as_ref().map(|c| c.arm.clone()),
            Some(ArmName("b".to_string()))
        );
        assert!(close(choice.map(|c| c.sampled_p).unwrap_or(f64::NAN), 0.9));
    }

    #[test]
    fn an_uncertain_arm_wins_when_its_draw_is_higher() {
        let mut proven = arm("proven");
        proven.posterior = Posterior {
            alpha: 10.0,
            beta: 1.0,
        };
        let mut unknown = arm("unknown");
        unknown.posterior = Posterior {
            alpha: 1.0,
            beta: 1.0,
        };
        assert!(proven.posterior.mean() > unknown.posterior.mean());

        let arms = vec![proven.clone(), unknown.clone()];
        let w = Weights {
            task_value: 1.0,
            dollar_weight: 0.0,
            latency_weight: 0.0,
        };

        let exploring = select(&arms, &[], NOW, &[], &w, &[0.2, 0.95]);
        assert_eq!(
            exploring.map(|c| c.arm),
            Some(ArmName("unknown".to_string()))
        );

        let exploiting = select(&arms, &[], NOW, &[], &w, &[0.9, 0.1]);
        assert_eq!(
            exploiting.map(|c| c.arm),
            Some(ArmName("proven".to_string()))
        );
    }

    #[test]
    fn equal_draws_go_to_the_cheaper_or_faster_arm() {
        let mut dear = arm("dear");
        dear.price_in = 5.0;
        dear.mean_latency_s = Some(1.0);
        let mut slow = arm("slow");
        slow.price_in = 0.0;
        slow.mean_latency_s = Some(30.0);
        let mut cheap_and_quick = arm("cheap_and_quick");
        cheap_and_quick.price_in = 0.0;
        cheap_and_quick.mean_latency_s = Some(2.0);

        let arms = vec![dear, slow, cheap_and_quick];
        let choice = select(&arms, &[], NOW, &[], &weights(), &[0.5, 0.5, 0.5]);
        assert_eq!(
            choice.map(|c| c.arm),
            Some(ArmName("cheap_and_quick".to_string()))
        );
    }

    #[test]
    fn zero_weights_leave_price_and_latency_out_of_the_score() {
        let mut dear = arm("dear");
        dear.price_in = 5.0;
        dear.mean_latency_s = Some(40.0);
        let mut cheap = arm("cheap");
        cheap.price_in = 0.0;
        cheap.mean_latency_s = Some(1.0);

        let arms = vec![dear.clone(), cheap.clone()];
        let w = Weights {
            task_value: 0.0,
            dollar_weight: 0.0,
            latency_weight: 0.0,
        };

        assert!(close(
            score(&dear, 0.7, &w, &arms),
            score(&cheap, 0.7, &w, &arms)
        ));
        let choice = select(&arms, &[], NOW, &[], &w, &[0.7, 0.7]);
        assert!(choice.map(|c| c.score).unwrap_or(f64::NAN).is_finite());
    }

    #[test]
    fn a_choice_reports_the_draw_that_won_it() {
        let arms = vec![arm("a"), arm("b")];
        let choice = select(&arms, &[], NOW, &[], &weights(), &[0.4, 0.6]);

        let Some(choice) = choice else {
            assert!(choice.is_some(), "expected a choice from an eligible arm");
            return;
        };
        assert_eq!(choice.arm, ArmName("b".to_string()));
        assert!(close(choice.sampled_p, 0.6));
        assert!(close(choice.score, score(&arms[1], 0.6, &weights(), &arms)));
    }

    #[test]
    fn router_types_survive_a_serde_round_trip() {
        let mut a = arm("a");
        a.capabilities = vec!["multiturn".to_string()];
        a.mean_latency_s = Some(3.5);
        a.parked_until = Some(42);

        let json = serde_json::to_string(&a).unwrap_or_default();
        let back: ArmInfo = serde_json::from_str(&json).unwrap_or_else(|_| arm("broken"));
        assert_eq!(back, a);

        let verdict = Ineligible::RedundantPaidRoute {
            free_arm: "free".to_string(),
        };
        let json = serde_json::to_string(&verdict).unwrap_or_default();
        let back: Ineligible =
            serde_json::from_str(&json).unwrap_or(Ineligible::Parked { until: 0 });
        assert_eq!(back, verdict);
    }
}
