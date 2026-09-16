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

    /// Spend per completion, or `None` when spend is unmeasured or there are
    /// no completions.
    pub fn usd_per_completion(&self) -> Option<f64> {
        let usd = self.usd.filter(|value| value.is_finite())?;
        if self.completed == 0 {
            return None;
        }
        let per_completion = usd / f64::from(self.completed);
        per_completion.is_finite().then_some(per_completion)
    }
}

#[derive(Default)]
struct PartialArm {
    runs: u32,
    completed: u32,
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
        if let Some(usd) = run.usd {
            add_usd(&mut partial.usd, usd);
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
pub fn frontier(arms: &[ArmCost], epsilon: f64) -> Vec<String> {
    let epsilon = effective_epsilon(epsilon);
    let usable: Vec<(usize, f64, f64)> = arms
        .iter()
        .enumerate()
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
pub fn totals(arms: &[ArmCost]) -> (f64, u32, u32) {
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
    (spend.unwrap_or(0.0), completed, runs)
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
