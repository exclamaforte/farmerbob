//! External priors for the Beta-Bernoulli bandit.
//!
//! Arms start life as `Beta(1, 1)`; an external prior replaces that cold start
//! with a stronger belief. The posterior folds observed outcomes on top of the
//! prior, and [`evidence_weight`] reports how much of what the router is
//! looking at is measurement rather than opinion — so a planning agent can tell
//! when a prior has actually been overruled by evidence instead of still
//! trusting a belief. Pure logic: no I/O.

/// A prior belief about one arm, from a source outside this project.
#[derive(Debug, Clone, PartialEq)]
pub struct PriorBelief {
    /// Which arm the belief describes.
    pub arm: String,
    /// Expected success rate, 0.0..=1.0.
    pub capability: f64,
    /// How much evidence this belief is worth, in pseudo-observations.
    /// 2.0 means "treat me as two prior runs" — easily overruled. 50.0 means the
    /// arm needs a long losing streak to fall.
    pub strength: f64,
    /// USD per million input tokens. 0.0 for a free route.
    pub price_in: f64,
    /// USD per million output tokens. 0.0 for a free route.
    pub price_out: f64,
}

/// Beta posterior for one arm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Posterior {
    pub alpha: f64,
    pub beta: f64,
}

impl Posterior {
    /// Mean of the Beta distribution: `alpha / (alpha + beta)`.
    ///
    /// Never returns NaN: a zero or non-finite total (unreachable from [`seed`],
    /// only from a hand-built value) reads as the uniform mean 0.5.
    pub fn mean(&self) -> f64 {
        let denom = self.alpha + self.beta;
        if denom == 0.0 || !denom.is_finite() {
            0.5
        } else {
            self.alpha / denom
        }
    }

    /// Total evidence behind this posterior, prior included.
    pub fn observations(&self) -> f64 {
        self.alpha + self.beta
    }
}

/// Turn a belief into the posterior an arm starts life with.
///
/// Maps capability and strength to `alpha = capability * strength` and
/// `beta = (1 - capability) * strength`, then adds the uniform `Beta(1, 1)`, so
/// a zero-strength belief degrades exactly to an unseeded arm and a capability
/// of exactly 0.0 or 1.0 never produces a zero parameter: an arm believed
/// certain must still be movable by evidence.
///
/// Errors on a capability outside 0.0..=1.0, a non-finite or negative strength,
/// or a non-finite or negative price.
pub fn seed(b: &PriorBelief) -> Result<Posterior, String> {
    if !b.capability.is_finite() || b.capability < 0.0 || b.capability > 1.0 {
        return Err("capability must be finite and in 0.0..=1.0".to_string());
    }
    if !b.strength.is_finite() || b.strength < 0.0 {
        return Err("strength must be finite and non-negative".to_string());
    }
    if !b.price_in.is_finite() || b.price_in < 0.0 {
        return Err("price_in must be finite and non-negative".to_string());
    }
    if !b.price_out.is_finite() || b.price_out < 0.0 {
        return Err("price_out must be finite and non-negative".to_string());
    }
    Ok(Posterior {
        alpha: b.capability * b.strength + 1.0,
        beta: (1.0 - b.capability) * b.strength + 1.0,
    })
}

/// Fold observed outcomes into a seeded posterior.
///
/// Neither parameter ever falls, and both saturate at `f64::MAX` rather than
/// overflowing to infinity.
pub fn update(p: Posterior, successes: u32, failures: u32) -> Posterior {
    Posterior {
        alpha: saturating_add(p.alpha, f64::from(successes)),
        beta: saturating_add(p.beta, f64::from(failures)),
    }
}

/// How far the posterior has moved from its prior, 0.0 at the seed and
/// approaching 1.0 as observations accumulate.
///
/// Concretely: the share of the current posterior's observations that came from
/// measurement rather than the prior. A stronger prior therefore needs more
/// observations to reach the same weight, which is what tells a planning agent
/// whether it is looking at a measurement or at someone's opinion.
pub fn evidence_weight(seeded: Posterior, current: Posterior) -> f64 {
    let total = current.observations();
    let prior = seeded.observations();
    if !total.is_finite() || !prior.is_finite() {
        return 1.0;
    }
    let empirical = (total - prior).max(0.0);
    if empirical == 0.0 {
        0.0
    } else {
        empirical / total
    }
}

/// Arms on the cost/capability Pareto frontier: no other arm is both cheaper and
/// at least as capable. Cost is `price_in + price_out` per million tokens.
///
/// A cost tie is broken in favour of the higher posterior mean (the lower-mean
/// arm is dominated). Posterior means that differ by less than `1e-9` count as
/// a tie, so float noise does not decide the ranking; a free arm (cost 0.0) is
/// included whenever it is not strictly dominated. Returned cheapest first.
pub fn pareto_frontier(arms: &[(String, f64, Posterior)]) -> Vec<String> {
    let mut kept: Vec<(String, f64, f64)> = vec![];
    for (i, arm) in arms.iter().enumerate() {
        let (name, cost, post) = arm;
        let mean = post.mean();
        let mut dominated = false;
        for (j, other) in arms.iter().enumerate() {
            if i == j {
                continue;
            }
            let (_, other_cost, other_post) = other;
            if dominates(*other_cost, other_post.mean(), *cost, mean) {
                dominated = true;
                break;
            }
        }
        if !dominated {
            kept.push((name.clone(), *cost, mean));
        }
    }

    // Cheapest first; equal costs order by higher posterior mean first.
    kept.sort_by(|a, b| {
        let a_cost = a.1;
        let b_cost = b.1;
        let a_mean = a.2;
        let b_mean = b.2;
        a_cost
            .partial_cmp(&b_cost)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b_mean.partial_cmp(&a_mean).unwrap_or(std::cmp::Ordering::Equal))
    });
    kept.iter().map(|(name, _, _)| name.clone()).collect()
}

/// Whether `a` (cost `cost_a`, mean `mean_a`) dominates `b`. Cheaper and at
/// least as capable dominates; on a cost tie the higher mean wins; mean
/// differences under `1e-9` are a tie, not a ranking.
fn dominates(cost_a: f64, mean_a: f64, cost_b: f64, mean_b: f64) -> bool {
    if cost_a > cost_b {
        return false;
    }
    if (mean_a - mean_b).abs() <= 1e-9 {
        cost_a < cost_b
    } else {
        mean_a > mean_b
    }
}

/// `a + b`, clamped to `f64::MAX` so finite inputs can never produce infinity.
fn saturating_add(a: f64, b: f64) -> f64 {
    let sum = a + b;
    if sum.is_finite() {
        sum
    } else {
        f64::MAX
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn belief(capability: f64, strength: f64) -> PriorBelief {
        PriorBelief {
            arm: "arm".into(),
            capability,
            strength,
            price_in: 0.0,
            price_out: 0.0,
        }
    }

    fn seeded_post(capability: f64, strength: f64) -> Posterior {
        seed(&belief(capability, strength)).unwrap()
    }

    fn arm(name: &str, cost: f64, post: Posterior) -> (String, f64, Posterior) {
        (name.into(), cost, post)
    }

    fn post(mean: f64) -> Posterior {
        Posterior { alpha: mean, beta: 1.0 - mean }
    }

    #[test]
    fn zero_strength_degrades_to_beta_1_1() {
        let p = seeded_post(0.4, 0.0);
        assert_eq!(p.alpha, 1.0);
        assert_eq!(p.beta, 1.0);
        assert_eq!(p.observations(), 2.0);
        assert!((p.mean() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn capability_one_still_yields_a_finite_movable_beta() {
        let p = seeded_post(1.0, 5.0);
        assert_eq!(p.alpha, 6.0);
        assert_eq!(p.beta, 1.0);
        assert!(p.alpha.is_finite() && p.beta.is_finite());
        let q = update(p, 0, 4);
        assert_eq!(q.beta, 5.0);
        assert!(q.beta.is_finite());
        assert!(q.mean() > 0.0 && q.mean() < 1.0, "evidence must be able to move a certain arm");
    }

    #[test]
    fn capability_zero_is_legal_and_movable_up() {
        let p = seeded_post(0.0, 5.0);
        assert_eq!(p.alpha, 1.0);
        assert_eq!(p.beta, 6.0);
        let q = update(p, 3, 0);
        assert!(q.mean() > p.mean());
    }

    #[test]
    fn long_losing_streak_overrules_a_strong_prior() {
        let seeded = seeded_post(0.9, 50.0);
        assert!(seeded.mean() > 0.8);
        let mut current = seeded;
        for _ in 0..100 {
            current = update(current, 0, 1);
        }
        assert!(
            current.mean() < 0.5,
            "100 straight failures must drag a strength-50 prior below 0.5"
        );
        assert!(current.mean() < seeded.mean());
    }

    #[test]
    fn evidence_weight_starts_at_zero_and_rises() {
        let seeded = seeded_post(0.8, 2.0);
        let mut current = seeded;
        let mut last = evidence_weight(seeded, current);
        assert_eq!(last, 0.0);
        for i in 0..20 {
            current = update(current, 1, 0);
            let w = evidence_weight(seeded, current);
            assert!(w > last, "weight must rise with each observation (step {i})");
            assert!(w <= 1.0);
            last = w;
        }
    }

    #[test]
    fn evidence_weight_never_exceeds_one_and_approaches_it() {
        let seeded = seeded_post(0.5, 5.0);
        let mut current = seeded;
        for _ in 0..10_000 {
            current = update(current, 1, 1);
        }
        let w = evidence_weight(seeded, current);
        assert!(w > 0.9, "after 10k observations the weight must approach 1.0");
        assert!(w <= 1.0);
    }

    #[test]
    fn evidence_weight_is_zero_only_at_the_seed() {
        let seeded = Posterior { alpha: 1.0, beta: 1.0 };
        assert_eq!(evidence_weight(seeded, seeded), 0.0);
        let one = update(seeded, 1, 0);
        let w = evidence_weight(seeded, one);
        assert!(w > 0.0 && w < 1.0);
    }

    #[test]
    fn stronger_prior_is_overruled_more_slowly() {
        let weak = seeded_post(0.5, 2.0);
        let strong = seeded_post(0.5, 50.0);
        let weak_after = update(weak, 5, 0);
        let strong_after = update(strong, 5, 0);
        assert!(
            evidence_weight(weak, weak_after) > evidence_weight(strong, strong_after),
            "the same five observations should weigh more against a weaker prior"
        );
    }

    #[test]
    fn update_folds_outcomes_in() {
        let p = seeded_post(0.5, 4.0);
        let q = update(p, 3, 1);
        assert_eq!(q.alpha, p.alpha + 3.0);
        assert_eq!(q.beta, p.beta + 1.0);
        assert_eq!(q.observations(), p.observations() + 4.0);
    }

    #[test]
    fn update_never_decreases_either_parameter() {
        let p = seeded_post(0.5, 4.0);
        let q = update(p, 0, 0);
        assert_eq!(q.alpha, p.alpha);
        assert_eq!(q.beta, p.beta);
    }

    #[test]
    fn update_saturates_instead_of_overflowing() {
        let p = Posterior { alpha: f64::MAX, beta: 1.0 };
        let q = update(p, u32::MAX, u32::MAX);
        assert!(q.alpha.is_finite(), "alpha must saturate, not overflow to infinity");
        assert!(q.beta.is_finite());
        assert_eq!(q.alpha, f64::MAX);
        assert_eq!(q.beta, 4_294_967_296.0);
    }

    #[test]
    fn nan_capability_is_rejected() {
        assert!(seed(&belief(f64::NAN, 2.0)).is_err());
    }

    #[test]
    fn capability_outside_unit_interval_is_rejected() {
        assert!(seed(&belief(1.1, 2.0)).is_err());
        assert!(seed(&belief(-0.1, 2.0)).is_err());
        assert!(seed(&belief(f64::INFINITY, 2.0)).is_err());
        assert!(seed(&belief(f64::NEG_INFINITY, 2.0)).is_err());
    }

    #[test]
    fn negative_or_non_finite_strength_is_rejected() {
        assert!(seed(&belief(0.5, -1.0)).is_err());
        assert!(seed(&belief(0.5, f64::INFINITY)).is_err());
        assert!(seed(&belief(0.5, f64::NAN)).is_err());
    }

    #[test]
    fn negative_or_non_finite_prices_are_rejected() {
        let mut b = belief(0.5, 2.0);
        b.price_in = -0.01;
        assert!(seed(&b).is_err());
        b = belief(0.5, 2.0);
        b.price_out = f64::NAN;
        assert!(seed(&b).is_err());
        b = belief(0.5, 2.0);
        b.price_out = -0.0 - 1.0;
        assert!(seed(&b).is_err());
    }

    #[test]
    fn finite_non_negative_prices_are_accepted() {
        let b = PriorBelief {
            arm: "paid".into(),
            capability: 0.7,
            strength: 3.0,
            price_in: 0.06,
            price_out: 0.12,
        };
        assert!(seed(&b).is_ok());
        assert!(seed(&belief(0.7, 3.0)).is_ok());
    }

    #[test]
    fn posterior_mean_and_observations_reflect_the_prior() {
        let p = seeded_post(0.7, 10.0);
        assert_eq!(p.observations(), 12.0);
        assert!((p.mean() - (0.7 * 10.0 + 1.0) / 12.0).abs() < 1e-9);
    }

    #[test]
    fn free_arm_appears_on_the_frontier() {
        let frontier = pareto_frontier(&[
            arm("paid", 5.0, post(0.9)),
            arm("mid", 3.0, post(0.7)),
            arm("free", 0.0, post(0.4)),
        ]);
        assert!(frontier.iter().any(|n| n == "free"));
        assert_eq!(frontier[0], "free".to_string(), "free is cheapest and must come first");
    }

    #[test]
    fn dominated_arm_does_not_appear_on_the_frontier() {
        let frontier = pareto_frontier(&[
            arm("good", 1.0, post(0.8)),
            arm("bad", 5.0, post(0.7)),
        ]);
        assert_eq!(frontier, vec!["good".to_string()]);
    }

    #[test]
    fn cost_tie_is_broken_by_posterior_mean() {
        let frontier = pareto_frontier(&[
            arm("weak", 2.0, post(0.3)),
            arm("strong", 2.0, post(0.8)),
        ]);
        assert_eq!(frontier, vec!["strong".to_string()]);
    }

    #[test]
    fn mean_difference_within_1e9_is_a_tie_not_a_ranking() {
        let frontier = pareto_frontier(&[
            arm("dear", 2.0, Posterior { alpha: 6.000000002, beta: 4.0 }),
            arm("cheap", 1.0, Posterior { alpha: 6.000000001, beta: 4.0 }),
        ]);
        assert_eq!(frontier, vec!["cheap".to_string()]);
    }

    #[test]
    fn free_arm_yields_to_a_better_free_arm() {
        let frontier = pareto_frontier(&[
            arm("free-good", 0.0, post(0.8)),
            arm("free-weak", 0.0, post(0.2)),
        ]);
        assert_eq!(frontier, vec!["free-good".to_string()]);
    }

    #[test]
    fn frontier_is_returned_cheapest_first() {
        let frontier = pareto_frontier(&[
            arm("mid", 2.0, post(0.5)),
            arm("dear", 3.0, post(0.9)),
            arm("cheap", 1.0, post(0.4)),
        ]);
        assert_eq!(frontier, vec!["cheap".to_string(), "mid".to_string(), "dear".to_string()]);
    }

    #[test]
    fn cheaper_but_less_capable_does_not_dominate() {
        let frontier = pareto_frontier(&[
            arm("capable", 3.0, post(0.9)),
            arm("cheap", 1.0, post(0.4)),
        ]);
        assert_eq!(frontier, vec!["cheap".to_string(), "capable".to_string()]);
    }
}