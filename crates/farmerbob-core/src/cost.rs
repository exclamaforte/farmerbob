//! Measured cost attribution and the cost/capability frontier.

use std::cmp::Ordering;
use std::collections::BTreeMap;

/// What one run cost and whether it counted.
#[derive(Debug, Clone, PartialEq)]
pub struct RunCost {
    /// The arm that performed the run.
    pub arm: String,
    /// The task attempted by the arm.
    pub task: String,
    /// Measured USD. `Some(0.0)` for a plan-based arm is a real zero;
    /// `None` is unmeasured.
    pub usd: Option<f64>,
    /// Measured token usage, when available.
    pub tokens: Option<u64>,
    /// Whether the run produced a passing deliverable.
    pub completed: bool,
    /// Whether the outcome says something about the arm.
    pub counts_for_arm: bool,
}

/// One arm's aggregate.
#[derive(Debug, Clone, PartialEq)]
pub struct ArmCost {
    /// The arm name.
    pub arm: String,
    /// Number of counted runs.
    pub runs: u32,
    /// Number of counted completed runs.
    pub completed: u32,
    /// Summed measured spend, or `None` when no counted run was measured.
    pub usd: Option<f64>,
    /// Summed measured token usage, or `None` when no counted run was measured.
    pub tokens: Option<u64>,
    /// Counted runs whose spend was not measured. Zero means the total is complete.
    pub unmeasured_runs: u32,
}

impl ArmCost {
    /// Completions over counted runs. `None` when no run counted.
    pub fn completion_rate(&self) -> Option<f64> {
        if self.runs == 0 {
            None
        } else {
            let rate = f64::from(self.completed) / f64::from(self.runs);
            rate.is_finite().then_some(rate)
        }
    }

    /// Spend per completion, or `None` when spend is unmeasured, the total is
    /// a lower bound (`unmeasured_runs > 0`), or there are no completions.
    pub fn usd_per_completion(&self) -> Option<f64> {
        if self.unmeasured_runs > 0 {
            return None;
        }
        let usd = self.usd.filter(|value| value.is_finite())?;
        if self.completed == 0 {
            return None;
        }
        let per_completion = usd / f64::from(self.completed);
        per_completion.is_finite().then_some(per_completion)
    }
}

/// Whether this arm's spend is a complete total rather than a lower bound.
///
/// `true` only when no counted run was unmeasured and at least one run counted:
/// an arm with no counted runs is unmeasured, not fully measured, and must not
/// be placed on a frontier.
pub fn fully_measured(arm: &ArmCost) -> bool {
    arm.unmeasured_runs == 0 && arm.runs > 0
}

#[derive(Default)]
struct PartialArm {
    runs: u32,
    completed: u32,
    unmeasured_runs: u32,
    usd: Option<f64>,
    tokens: Option<u64>,
}

fn add_usd(total: &mut Option<f64>, value: f64) {
    if !value.is_finite() {
        return;
    }
    *total = Some(match *total {
        None => value,
        Some(previous) => {
            let sum = previous + value;
            if sum.is_finite() {
                sum
            } else if value.is_sign_negative() == previous.is_sign_negative() {
                if value.is_sign_negative() {
                    -f64::MAX
                } else {
                    f64::MAX
                }
            } else {
                0.0
            }
        }
    });
}

/// Aggregate counted runs per arm, sorted by arm name.
pub fn aggregate(runs: &[RunCost]) -> Vec<ArmCost> {
    let mut partials = BTreeMap::<String, PartialArm>::new();
    for run in runs {
        let partial = partials.entry(run.arm.clone()).or_default();
        if !run.counts_for_arm {
            continue;
        }
        partial.runs = partial.runs.saturating_add(1);
        if run.completed {
            partial.completed = partial.completed.saturating_add(1);
        }
        match run.usd {
            None => partial.unmeasured_runs = partial.unmeasured_runs.saturating_add(1),
            Some(usd) => add_usd(&mut partial.usd, usd),
        }
        if let Some(tokens) = run.tokens {
            partial.tokens = Some(partial.tokens.unwrap_or(0).saturating_add(tokens));
        }
    }

    partials
        .into_iter()
        .map(|(arm, partial)| ArmCost {
            arm,
            runs: partial.runs,
            completed: partial.completed,
            usd: partial.usd,
            tokens: partial.tokens,
            unmeasured_runs: partial.unmeasured_runs,
        })
        .collect()
}

fn effective_epsilon(epsilon: f64) -> f64 {
    if epsilon.is_finite() && epsilon >= 0.0 {
        epsilon
    } else {
        0.0
    }
}

fn no_more(a: f64, b: f64, epsilon: f64) -> bool {
    a <= b + epsilon
}

fn strictly_better(a: f64, b: f64, epsilon: f64) -> bool {
    a < b - epsilon
}

/// Return the arms not dominated by another arm, cheapest first.
///
/// Only arms that are [`fully_measured`] are considered: a partial spend is a
/// lower bound, and a lower bound cannot be shown not to dominate.
pub fn frontier(arms: &[ArmCost], epsilon: f64) -> Vec<String> {
    let epsilon = effective_epsilon(epsilon);
    let usable: Vec<(usize, f64, f64)> = arms
        .iter()
        .enumerate()
        .filter(|(_, arm)| fully_measured(arm))
        .filter_map(|(index, arm)| Some((index, arm.usd_per_completion()?, arm.completion_rate()?)))
        .collect();

    let mut frontier = usable
        .iter()
        .filter(|&&(index, cost, rate)| {
            !usable.iter().any(|&(other_index, other_cost, other_rate)| {
                other_index != index
                    && no_more(other_cost, cost, epsilon)
                    && no_more(rate, other_rate, epsilon)
                    && (strictly_better(other_cost, cost, epsilon)
                        || strictly_better(other_rate, rate, epsilon))
            })
        })
        .map(|&(index, cost, rate)| (index, cost, rate))
        .collect::<Vec<_>>();

    frontier.sort_by(|&(a_index, a_cost, a_rate), &(b_index, b_cost, b_rate)| {
        if (a_cost - b_cost).abs() <= epsilon {
            if (a_rate - b_rate).abs() <= epsilon {
                arms[a_index].arm.cmp(&arms[b_index].arm)
            } else {
                b_rate.partial_cmp(&a_rate).unwrap_or(Ordering::Equal)
            }
        } else {
            a_cost.partial_cmp(&b_cost).unwrap_or(Ordering::Equal)
        }
    });
    frontier
        .into_iter()
        .map(|(index, _, _)| arms[index].arm.clone())
        .collect()
}

/// Return total measured spend, completions, and counted runs.
/// Total measured spend, completions and counted runs.
///
/// Spend is `None` when NOTHING was measured, which is not the same as zero spend. This
/// function used to accumulate into an `Option` with care and then return
/// `spend.unwrap_or(0.0)` — a single `unwrap_or` at the boundary that discarded the whole
/// distinction the surrounding type system was built to keep. It went unnoticed because the
/// module had no caller; `fb pareto` is the first, and found it immediately.
///   (bead farmerbob-jd2.5)
pub fn totals(arms: &[ArmCost]) -> (Option<f64>, u32, u32) {
    let mut spend = None;
    let mut completed: u32 = 0;
    let mut runs: u32 = 0;
    for arm in arms {
        if let Some(usd) = arm.usd {
            add_usd(&mut spend, usd);
        }
        completed = completed.saturating_add(arm.completed);
        runs = runs.saturating_add(arm.runs);
    }
    (spend, completed, runs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arm(name: &str, usd: Option<f64>, runs: u32, completed: u32) -> ArmCost {
        ArmCost {
            arm: name.to_string(),
            runs,
            completed,
            usd,
            tokens: None,
            unmeasured_runs: 0,
        }
    }

    #[test]
    fn measured_zero_is_on_frontier_and_unmeasured_arm_is_not() {
        let arms = vec![arm("free", Some(0.0), 1, 1), arm("unknown", None, 1, 1)];
        assert_eq!(frontier(&arms, 0.0), vec!["free"]);
    }

    #[test]
    fn zero_completions_gives_none_rather_than_infinity() {
        let cost = arm("failed", Some(4.0), 2, 0);
        assert_eq!(cost.usd_per_completion(), None);
    }

    #[test]
    fn all_excluded_arm_has_rate_none_not_zero() {
        let result = aggregate(&[RunCost {
            arm: "blocked".into(),
            task: "t".into(),
            usd: Some(2.0),
            tokens: Some(5),
            completed: true,
            counts_for_arm: false,
        }]);
        assert_eq!(result[0].runs, 0);
        assert_eq!(result[0].completion_rate(), None);
    }

    #[test]
    fn one_unmeasured_run_does_not_poison_a_sum() {
        let result = aggregate(&[
            RunCost {
                arm: "a".into(),
                task: "1".into(),
                usd: Some(1.25),
                tokens: None,
                completed: true,
                counts_for_arm: true,
            },
            RunCost {
                arm: "a".into(),
                task: "2".into(),
                usd: None,
                tokens: None,
                completed: false,
                counts_for_arm: true,
            },
        ]);
        assert_eq!(result[0].usd, Some(1.25));
    }

    #[test]
    fn two_free_arms_are_ordered_by_completion_rate() {
        let arms = vec![arm("slow", Some(0.0), 2, 1), arm("fast", Some(0.0), 2, 2)];
        assert_eq!(frontier(&arms, 0.0), vec!["fast", "slow"]);
    }

    #[test]
    fn dominated_arm_is_absent() {
        let arms = vec![
            arm("best", Some(1.0), 2, 2),
            arm("dominated", Some(2.0), 2, 1),
        ];
        assert_eq!(frontier(&arms, 0.0), vec!["best"]);
    }

    #[test]
    fn epsilon_absorbs_float_noise() {
        let arms = vec![arm("a", Some(1.0), 1, 1), arm("b", Some(1.0 + 1e-10), 1, 1)];
        assert_eq!(frontier(&arms, 1e-9), vec!["a", "b"]);
    }
}

#[cfg(test)]
mod unmeasured_spend {
    use super::*;

    fn arm_of(name: &str, usd: Option<f64>) -> ArmCost {
        ArmCost { arm: name.into(), runs: 3, completed: 2, usd, tokens: None, unmeasured_runs: 0 }
    }

    /// Instance twelve of this project's recurring failure, found inside the module written
    /// to prevent it, by the act of giving that module its first caller.
    #[test]
    fn a_field_nothing_measured_reports_none_not_zero() {
        let none_measured = [arm_of("codex-luna", None), arm_of("glm-53-flash", None)];
        let (spend, completed, runs) = totals(&none_measured);
        assert_eq!(spend, None, "unmeasured spend must not read as $0.00");
        assert_eq!((completed, runs), (4, 6), "the run counts are still real");
    }

    /// A field that genuinely spent nothing is Some(0.0), and must stay distinguishable.
    #[test]
    fn a_field_that_really_spent_nothing_reports_some_zero() {
        let free = [arm_of("or-inkling", Some(0.0))];
        assert_eq!(totals(&free).0, Some(0.0));
        assert_ne!(totals(&free).0, totals(&[arm_of("codex-luna", None)]).0);
    }

    /// One measured arm among unmeasured ones yields that arm's spend, not a padded sum.
    #[test]
    fn partial_measurement_sums_only_what_was_measured() {
        let mixed = [arm_of("a", Some(1.5)), arm_of("b", None), arm_of("c", Some(0.25))];
        assert_eq!(totals(&mixed).0, Some(1.75));
    }
}

#[cfg(test)]
mod partial_total_tests {
    use super::*;

    fn run(arm: &str, usd: Option<f64>, completed: bool, counts: bool) -> RunCost {
        RunCost {
            arm: arm.into(),
            task: "t".into(),
            usd,
            tokens: None,
            completed,
            counts_for_arm: counts,
        }
    }

    fn hand(arm: &str, usd: Option<f64>, runs: u32, completed: u32, unmeasured: u32) -> ArmCost {
        ArmCost {
            arm: arm.into(),
            runs,
            completed,
            usd,
            tokens: None,
            unmeasured_runs: unmeasured,
        }
    }

    // Rule 1: aggregate counts exactly the counted runs whose spend was None.
    #[test]
    fn rule_1_aggregate_counts_unmeasured_counted_runs() {
        let result = aggregate(&[
            run("a", Some(1.0), true, true),
            run("a", None, true, true),
            run("a", None, false, true),
            run("a", None, true, false),
            run("a", Some(2.0), true, false),
            run("b", Some(1.0), true, true),
            run("b", Some(2.0), false, true),
        ]);
        let a = &result[0];
        assert_eq!(a.arm, "a");
        assert_eq!(a.runs, 3);
        assert_eq!(a.unmeasured_runs, 2, "only counted runs with usd None are counted");
        assert_eq!(a.usd, Some(1.0));
        let b = &result[1];
        assert_eq!(b.arm, "b");
        assert_eq!(b.runs, 2);
        assert_eq!(b.unmeasured_runs, 0, "fully priced arms report zero unmeasured");
    }

    // Rule 2: fully_measured iff unmeasured_runs == 0 AND runs > 0.
    #[test]
    fn rule_2_fully_measured_requires_no_unmeasured_and_some_runs() {
        assert!(fully_measured(&hand("a", Some(1.0), 1, 1, 0)));
        assert!(!fully_measured(&hand("a", Some(1.0), 2, 1, 1)));
        assert!(
            !fully_measured(&hand("a", None, 0, 0, 0)),
            "an arm with no counted runs is unmeasured, not fully measured"
        );
        assert!(!fully_measured(&hand("a", Some(1.0), 0, 0, 2)));
    }

    // Rule 3: a partial total is never a cost per completion.
    #[test]
    fn rule_3_usd_per_completion_none_when_partial() {
        assert_eq!(
            hand("a", Some(6.0), 4, 3, 1).usd_per_completion(),
            None,
            "a lower bound divided by all completions is not a cost per completion"
        );
        assert_eq!(hand("a", Some(6.0), 4, 0, 1).usd_per_completion(), None);
        assert_eq!(hand("a", None, 4, 3, 1).usd_per_completion(), None);
        // Existing conditions still hold for complete totals.
        assert_eq!(hand("a", Some(6.0), 3, 3, 0).usd_per_completion(), Some(2.0));
        assert_eq!(hand("a", None, 3, 3, 0).usd_per_completion(), None);
        assert_eq!(hand("a", Some(6.0), 3, 0, 0).usd_per_completion(), None);
    }

    // Rule 4: partial arms never appear on the frontier, whatever their rates.
    #[test]
    fn rule_4_frontier_excludes_partially_measured_arms() {
        // "cheap-partial" would dominate "honest" if its lower bound were trusted.
        let arms = vec![
            hand("cheap-partial", Some(0.5), 3, 3, 1),
            hand("honest", Some(9.0), 3, 1, 0),
        ];
        assert_eq!(frontier(&arms, 0.0), vec!["honest"]);

        // An excluded arm is absent even when it is the only candidate.
        assert!(frontier(&[hand("partial", Some(0.5), 3, 3, 1)], 0.0).is_empty());

        // Reaching the same state through aggregate, not a hand-built literal.
        let aggregated = aggregate(&[
            run("cheap-partial", Some(0.5), true, true),
            run("cheap-partial", Some(1.0), true, true),
            run("cheap-partial", None, true, true),
        ]);
        assert!(frontier(&aggregated, 0.0).is_empty());
    }

    // Rule 5: totals is accounting, not comparison: partial spend still counts.
    #[test]
    fn rule_5_totals_includes_partial_arms() {
        let arms = vec![
            hand("partial", Some(2.0), 3, 1, 1),
            hand("complete", Some(1.5), 2, 2, 0),
        ];
        assert_eq!(totals(&arms), (Some(3.5), 3, 5));
        // The very same arms the frontier comparison must separate from accounting:
        assert_eq!(frontier(&arms, 0.0), vec!["complete"]);
    }

    // Boundary: every counted run unmeasured.
    #[test]
    fn boundary_all_counted_runs_unmeasured() {
        let result = aggregate(&[
            run("a", None, true, true),
            run("a", None, false, true),
        ]);
        let a = &result[0];
        assert_eq!(a.usd, None);
        assert_eq!(a.unmeasured_runs, 2);
        assert!(!fully_measured(a));
        assert_eq!(a.usd_per_completion(), None);
        assert!(!frontier(&result, 0.0).contains(&"a".to_string()));
    }

    // Boundary: an arm whose runs are all excluded has unmeasured_runs 0 yet is
    // not fully measured.
    #[test]
    fn boundary_all_runs_excluded_is_not_fully_measured() {
        let result = aggregate(&[run("b", Some(5.0), true, false)]);
        let b = &result[0];
        assert_eq!(b.runs, 0);
        assert_eq!(b.unmeasured_runs, 0);
        assert!(!fully_measured(b), "runs == 0 means unmeasured, not fully measured");
        assert!(!frontier(&result, 0.0).contains(&"b".to_string()));
    }

    // Boundary: empty input.
    #[test]
    fn boundary_empty_input() {
        let result = aggregate(&[]);
        assert!(result.is_empty());
        assert!(frontier(&result, 0.0).is_empty());
        assert!(frontier(&[], 0.5).is_empty());
    }

    // Boundary: zero completions with full measurement.
    #[test]
    fn boundary_zero_completions_fully_measured_but_not_on_frontier() {
        let result = aggregate(&[run("a", Some(1.0), false, true)]);
        let a = &result[0];
        assert!(fully_measured(a));
        assert_eq!(a.usd_per_completion(), None);
        assert!(!frontier(&result, 0.0).contains(&"a".to_string()));
    }

    // Composition: one arm per distinct name in the input, sorted, none dropped,
    // even when partially measured or wholly excluded.
    #[test]
    fn composition_one_entry_per_distinct_arm_sorted() {
        let result = aggregate(&[
            run("c", Some(1.0), true, false),
            run("b", None, false, true),
            run("a", Some(2.0), true, true),
            run("b", Some(1.0), true, true),
        ]);
        assert_eq!(
            result.iter().map(|a| a.arm.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
        assert_eq!(result[0].unmeasured_runs, 0);
        assert_eq!(result[1].unmeasured_runs, 1);
        assert_eq!(result[2].runs, 0);
        assert_eq!(result[2].unmeasured_runs, 0);
    }
}
