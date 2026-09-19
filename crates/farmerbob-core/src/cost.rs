//! Measured cost attribution and the cost/capability frontier.
//!
//! This module keeps value, absence, and billing class distinct. In particular,
//! an observed zero is not the same fact as a cost that was never measured.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::limit_signal::parse_reset;
use crate::measurement::{Absent, Measurement};
use crate::pricing::Billing;

/// What one run cost, how it was billed, and whether it counted.
#[derive(Debug, Clone, PartialEq)]
pub struct RunCost {
    /// The arm that performed the run.
    pub arm: String,
    /// The task attempted by the arm.
    pub task: String,
    /// What this run cost, or why the cost is not known.
    pub usd: Measurement<f64>,
    /// How this run was billed.
    pub billing: Billing,
    /// The token usage reported for this run.
    pub tokens: Measurement<u64>,
    /// Whether the run produced a passing deliverable.
    pub completed: bool,
    /// Whether the outcome says something about the arm.
    pub counts_for_arm: bool,
}

/// One arm's aggregate cost and ability measurements.
#[derive(Debug, Clone, PartialEq)]
pub struct ArmCost {
    /// The arm name.
    pub arm: String,
    /// Number of input rows for this arm, including uncounted rows.
    pub runs: u32,
    /// Number of counted completed runs.
    pub completed: u32,
    /// Total spend, or why the total is not known.
    pub usd: Measurement<f64>,
    /// Billing class selected for this arm.
    pub billing: Billing,
    /// Total counted token usage, or why it is not known.
    pub tokens: Measurement<u64>,
    /// Counted runs whose spend was not measured.
    pub unmeasured_runs: u32,
}

impl ArmCost {
    /// Completions over counted runs, or a missing measurement when there were
    /// no counted runs.
    pub fn completion_rate(&self) -> Measurement<f64> {
        if self.runs == 0 {
            Measurement::nothing_to_measure("an arm with no runs has no completion rate")
        } else {
            Measurement::observed(f64::from(self.completed) / f64::from(self.runs))
        }
    }

    /// Spend per completion, preserving a missing spend's exact reason.
    pub fn usd_per_completion(&self) -> Measurement<f64> {
        let usd = match &self.usd {
            Measurement::Missing(absent) => return Measurement::Missing(absent.clone()),
            Measurement::Observed(value) => *value,
        };
        if self.unmeasured_runs > 0 {
            return Measurement::instrument_failed(
                "the arm has unmeasured counted runs, so its total is only a lower bound",
            );
        }
        if self.completed == 0 {
            return Measurement::nothing_to_measure(
                "an arm with no completions has no spend per completion",
            );
        }
        let per_completion = usd / f64::from(self.completed);
        if per_completion.is_finite() {
            Measurement::observed(per_completion)
        } else {
            Measurement::untrusted("the spend per completion is not finite")
        }
    }
}

/// Whether an arm has a complete, trustworthy cost total.
pub fn fully_measured(arm: &ArmCost) -> bool {
    if arm.runs == 0 || arm.unmeasured_runs != 0 {
        return false;
    }
    let Measurement::Observed(usd) = arm.usd else {
        return false;
    };
    if !usd.is_finite() {
        return false;
    }
    match &arm.billing {
        Billing::Free => usd == 0.0,
        Billing::Metered { .. } => true,
        Billing::Subscription => usd == 0.0,
        Billing::MeteredUnpriced | Billing::Unknown => false,
    }
}

#[derive(Default)]
struct PartialArm {
    runs: u32,
    completed: u32,
    unmeasured_runs: u32,
    usd_sum: Option<f64>,
    usd_absent: Option<Absent>,
    token_sum: Option<u64>,
    token_absent: Option<Absent>,
    billing: Option<Billing>,
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

fn record_billing(partial: &mut PartialArm, billing: &Billing) {
    match &partial.billing {
        None => partial.billing = Some(billing.clone()),
        Some(existing) if existing == billing => {}
        Some(_) => partial.billing = Some(Billing::Unknown),
    }
}

fn record_usd(partial: &mut PartialArm, usd: &Measurement<f64>) {
    match usd {
        Measurement::Observed(value) if value.is_finite() => {
            if partial.usd_absent.is_none() {
                add_usd(&mut partial.usd_sum, *value);
            }
        }
        Measurement::Observed(_) => {
            if partial.usd_absent.is_none() {
                partial.usd_absent = Some(Absent::Untrusted {
                    reason: "the run reported a non-finite USD cost".to_string(),
                });
            }
        }
        Measurement::Missing(absent) => {
            if partial.usd_absent.is_none() {
                partial.usd_absent = Some(absent.clone());
            }
        }
    }
}

fn record_tokens(partial: &mut PartialArm, tokens: &Measurement<u64>) {
    match tokens {
        Measurement::Observed(value) => {
            if partial.token_absent.is_none() {
                let total = partial.token_sum.unwrap_or(0).saturating_add(*value);
                partial.token_sum = Some(total);
            }
        }
        Measurement::Missing(absent) => {
            if partial.token_absent.is_none() {
                partial.token_absent = Some(absent.clone());
            }
        }
    }
}

fn aggregate_usd(partial: &PartialArm) -> Measurement<f64> {
    match &partial.usd_absent {
        Some(absent) => Measurement::Missing(absent.clone()),
        None => match partial.usd_sum {
            Some(sum) => Measurement::Observed(sum),
            None => Measurement::not_attempted(),
        },
    }
}

fn aggregate_tokens(partial: &PartialArm) -> Measurement<u64> {
    match &partial.token_absent {
        Some(absent) => Measurement::Missing(absent.clone()),
        None => match partial.token_sum {
            Some(sum) => Measurement::Observed(sum),
            None => Measurement::not_attempted(),
        },
    }
}

/// Aggregate runs by arm name.
///
/// Every input row contributes to `runs`, including uncounted rows. Only
/// counted rows contribute completion, USD, and token totals. Mixed billing
/// classes resolve conservatively to `Billing::Unknown`; a missing counted
/// USD or token measurement makes that aggregate measurement missing rather
/// than presenting a partial total.
pub fn aggregate(runs: &[RunCost]) -> Vec<ArmCost> {
    let mut partials = BTreeMap::<String, PartialArm>::new();
    for run in runs {
        let partial = partials.entry(run.arm.clone()).or_default();
        partial.runs = partial.runs.saturating_add(1);
        if !run.counts_for_arm {
            continue;
        }
        if run.completed {
            partial.completed = partial.completed.saturating_add(1);
        }
        record_billing(partial, &run.billing);
        record_usd(partial, &run.usd);
        if matches!(run.usd, Measurement::Missing(_)) {
            partial.unmeasured_runs = partial.unmeasured_runs.saturating_add(1);
        }
        record_tokens(partial, &run.tokens);
    }

    partials
        .into_iter()
        .map(|(arm, partial)| ArmCost {
            arm,
            runs: partial.runs,
            completed: partial.completed,
            usd: aggregate_usd(&partial),
            billing: partial.billing.clone().unwrap_or(Billing::Unknown),
            tokens: aggregate_tokens(&partial),
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

fn strictly_better_rate(a: f64, b: f64, epsilon: f64) -> bool {
    a > b + epsilon
}

/// Return the non-dominated, fully measured arms, cheapest first.
pub fn frontier(arms: &[ArmCost], epsilon: f64) -> Vec<String> {
    let epsilon = effective_epsilon(epsilon);
    let usable: Vec<(usize, f64, f64)> = arms
        .iter()
        .enumerate()
        .filter(|(_, arm)| fully_measured(arm))
        .filter_map(|(index, arm)| {
            let Measurement::Observed(cost) = arm.usd_per_completion() else {
                return None;
            };
            let Measurement::Observed(rate) = arm.completion_rate() else {
                return None;
            };
            Some((index, cost, rate))
        })
        .collect();

    let mut frontier = usable
        .iter()
        .filter(|&&(index, cost, rate)| {
            !usable.iter().any(|&(other_index, other_cost, other_rate)| {
                other_index != index
                    && no_more(other_cost, cost, epsilon)
                    && no_more(rate, other_rate, epsilon)
                    && (strictly_better(other_cost, cost, epsilon)
                        || strictly_better_rate(other_rate, rate, epsilon))
            })
        })
        .copied()
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

/// Return total spend, completions, and counted runs.
///
/// Counts are always observed. Spend is observed only when every supplied arm
/// has an observed total; the first missing reason is preserved otherwise.
pub fn totals(arms: &[ArmCost]) -> (Measurement<f64>, u32, u32) {
    let mut spend = None;
    let mut missing = None;
    let mut completed = 0_u32;
    let mut runs = 0_u32;
    for arm in arms {
        completed = completed.saturating_add(arm.completed);
        runs = runs.saturating_add(arm.runs);
        match &arm.usd {
            Measurement::Observed(value) if value.is_finite() && missing.is_none() => {
                add_usd(&mut spend, *value);
            }
            Measurement::Observed(_) => {
                if missing.is_none() {
                    missing = Some(Absent::Untrusted {
                        reason: "an arm reported a non-finite USD total".to_string(),
                    });
                }
            }
            Measurement::Missing(absent) => {
                if missing.is_none() {
                    missing = Some(absent.clone());
                }
            }
        }
    }
    let spend = match missing {
        Some(absent) => Measurement::Missing(absent),
        None if arms.is_empty() => Measurement::not_attempted(),
        None => Measurement::Observed(spend.unwrap_or(0.0)),
    };
    (spend, completed, runs)
}

const CODEX_TOKEN_MARKER: &str = "tokens used";

/// A launcher whose token log format this module knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launcher {
    /// Anthropic's CLI.
    Claude,
    /// OpenAI's CLI.
    Codex,
    /// Google's CLI.
    Gemini,
    /// The Ori launcher.
    Ori,
    /// OpenCode and its OpenRouter routes.
    Opencode,
    /// The Agy launcher.
    Agy,
    /// An unrecognised launcher.
    Unknown,
}

/// Map a registry launcher name to the single launcher table.
pub fn launcher_from_name(name: &str) -> Launcher {
    match name {
        "claude" => Launcher::Claude,
        "codex" => Launcher::Codex,
        "gemini" => Launcher::Gemini,
        "ori" => Launcher::Ori,
        "opencode" => Launcher::Opencode,
        "agy" => Launcher::Agy,
        _ => Launcher::Unknown,
    }
}

/// Token counts broken out by billing dimension.
#[derive(Debug, Clone, PartialEq)]
pub struct Tokens {
    /// Fresh input tokens.
    pub input: Measurement<u64>,
    /// Output tokens.
    pub output: Measurement<u64>,
    /// Prompt-cache read tokens.
    pub cache_read: Measurement<u64>,
    /// A single figure for a launcher that reports no dimension breakdown at all.
    ///
    /// NOT a total of the three above -- it is what `codex exec` means by "tokens used",
    /// and adding it to them would double count. Exactly one side is ever observed: a
    /// launcher either reports dimensions or reports this.
    ///
    /// The spec for this module pinned Tokens at three fields and forbade a fourth, on the
    /// reasoning that a stored total is a fourth place for three numbers to disagree. That
    /// was right about a total and wrong about this: codex reports only a combined count, so
    /// the three-field shape forced `input` and `output` to Missing(Untrusted) and the
    /// Pareto board recovered nothing for every codex arm. The critic predicted exactly that
    /// -- "downstream callers expecting token counts from Codex runs will recover no usable
    /// metrics" -- before it happened, and it happened.
    pub combined: Measurement<u64>,
}

fn no_token_dimension(launcher: Launcher, dimension: &str) -> Measurement<u64> {
    Measurement::nothing_to_measure(&format!(
        "{launcher:?} does not report a separate {dimension} token count"
    ))
}

fn parse_count(payload: &str, dimension: &str) -> Measurement<u64> {
    let digits = payload.replace(',', "");
    let digits = digits.trim();
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Measurement::instrument_failed(&format!(
            "the {dimension} token count is not a non-negative integer"
        ));
    }
    match digits.parse::<u64>() {
        Ok(value) => Measurement::observed(value),
        Err(_) => {
            Measurement::untrusted(&format!("the {dimension} token count does not fit in u64"))
        }
    }
}

fn labelled_count(log: &str, labels: &[&str], dimension: &str) -> Measurement<u64> {
    let mut matched = false;
    let mut last_count = None;
    for line in log.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        for label in labels {
            if let Some(rest) = lower.strip_prefix(label) {
                matched = true;
                let original_rest = &trimmed[trimmed.len() - rest.len()..];
                let payload = original_rest
                    .trim_start_matches([' ', '\t', ':', '='])
                    .trim();
                if let Measurement::Observed(count) = parse_count(payload, dimension) {
                    last_count = Some(count);
                }
                break;
            }
        }
    }
    match last_count {
        Some(count) => Measurement::observed(count),
        None if matched => Measurement::nothing_to_measure(&format!(
            "the log contains a {dimension} label but no parseable token count"
        )),
        None => {
            Measurement::nothing_to_measure(&format!("the log contains no {dimension} token label"))
        }
    }
}

fn codex_combined_count(log: &str) -> Measurement<u64> {
    let lines: Vec<&str> = log.lines().collect();
    let marker = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim() == CODEX_TOKEN_MARKER)
        .map(|(index, _)| index)
        .next_back();
    match marker {
        None => Measurement::nothing_to_measure(
            "codex log contains no `tokens used` line; the run reported no token count",
        ),
        Some(index) => match lines.get(index + 1) {
            None => Measurement::instrument_failed(
                "codex printed `tokens used` as its final line and no count follows it",
            ),
            Some(payload) => parse_count(payload, "combined"),
        },
    }
}

/// Read token dimensions from a launcher log.
///
/// Explicit `input`, `output`, and `cache_read`/`cache read` labels are
/// recognised case-insensitively. The legacy Codex `tokens used` line reports
/// only a combined total, so it is not assigned to an invented billing class;
/// the three dimensions remain missing unless separate labels are present.
pub fn tokens_from_log(launcher: Launcher, log: &str) -> Tokens {
    if launcher == Launcher::Unknown {
        return Tokens {
            input: no_token_dimension(launcher, "input"),
            output: no_token_dimension(launcher, "output"),
            cache_read: no_token_dimension(launcher, "cache-read"),
            combined: no_token_dimension(launcher, "combined"),
        };
    }

    let mut input = labelled_count(log, &["input tokens", "input_tokens", "input"], "input");
    let mut output = labelled_count(log, &["output tokens", "output_tokens", "output"], "output");
    let cache_read = labelled_count(
        log,
        &[
            "cache_read tokens",
            "cache read tokens",
            "cache_read",
            "cache read",
        ],
        "cache-read",
    );

    let mut combined_total = no_token_dimension(launcher, "combined");
    if launcher == Launcher::Codex
        && matches!(input, Measurement::Missing(Absent::NothingToMeasure { .. }))
        && matches!(
            output,
            Measurement::Missing(Absent::NothingToMeasure { .. })
        )
    {
        let combined = codex_combined_count(log);
        if !matches!(
            combined,
            Measurement::Missing(Absent::NothingToMeasure { .. })
        ) {
            input = Measurement::Missing(Absent::Untrusted {
                reason: "codex reported a combined token total, not separate input tokens"
                    .to_string(),
            });
            output = Measurement::Missing(Absent::Untrusted {
                reason: "codex reported a combined token total, not separate output tokens"
                    .to_string(),
            });
            // Keep the figure codex DID report. Discarding it left the Pareto board with no
            // token count for any codex arm -- honest about the breakdown and silent about
            // the measurement that existed.
            combined_total = combined;
        }
    }

    Tokens {
        input,
        output,
        cache_read,
        combined: combined_total,
    }
}

fn refusal_signal(launcher: Launcher, exit_code: i32, log: &str) -> bool {
    let text = crate::limit_signal::normalise(log).to_ascii_lowercase();
    let known_marker = [
        "individual quota reached",
        "quota exceeded",
        "rate limit exceeded",
        "usage limit reached",
        "key limit exceeded",
        "token limit exceeded",
        "resource exhausted",
        "too many requests",
        "429",
    ]
    .iter()
    .any(|marker| text.contains(marker));
    let launcher_specific = match launcher {
        Launcher::Claude
        | Launcher::Codex
        | Launcher::Gemini
        | Launcher::Ori
        | Launcher::Opencode
        | Launcher::Agy => known_marker,
        Launcher::Unknown => false,
    };
    exit_code == 429 || (exit_code != 0 && launcher_specific)
}

/// Decide whether an observed run warrants parking its arm.
///
/// This pure decision recognises the common provider refusal markers and exit
/// status 429. `parse_reset` is called with `now == 0` because this signature
/// has no observation timestamp: relative reset values are therefore returned
/// as their stated seconds, while absolute reset formats remain absolute.
pub fn park_for(launcher: Launcher, exit_code: i32, log: &str) -> Measurement<u64> {
    if !refusal_signal(launcher, exit_code, log) {
        return Measurement::instrument_failed(
            "the observation did not match a recognised provider refusal",
        );
    }
    match parse_reset(log, 0) {
        Some(reset) => Measurement::observed(reset),
        None => {
            Measurement::nothing_to_measure("the provider refusal did not state a reset interval")
        }
    }
}

/// One reconciliation result between our spend and a provider's spend.
#[derive(Debug, Clone, PartialEq)]
pub enum Reconciliation {
    /// The two totals agree within tolerance.
    Agrees { ours_usd: f64, theirs_usd: f64 },
    /// The two finite totals differ by more than tolerance.
    Differs {
        /// Our total.
        ours_usd: f64,
        /// The provider's total.
        theirs_usd: f64,
        /// `theirs_usd - ours_usd`.
        residual_usd: f64,
        /// Residual divided by the provider's total.
        fraction: f64,
    },
    /// No comparison was made.
    Unchecked {
        /// Why comparison was unavailable.
        missing: String,
    },
}

/// Compare our attributable total with a provider's reported total.
pub fn reconcile(
    ours_usd: Option<f64>,
    theirs_usd: Option<f64>,
    tolerance_fraction: f64,
) -> Reconciliation {
    if !tolerance_fraction.is_finite() || tolerance_fraction < 0.0 {
        return Reconciliation::Unchecked {
            missing: format!(
                "tolerance_fraction {tolerance_fraction} is not a usable fraction; no comparison was made"
            ),
        };
    }
    let ours = match ours_usd {
        Some(value) if value.is_finite() => value,
        Some(value) => {
            return Reconciliation::Unchecked {
                missing: format!("ours_usd is {value}, which is not finite"),
            };
        }
        None => {
            return Reconciliation::Unchecked {
                missing: "ours_usd is unavailable".to_string(),
            };
        }
    };
    let theirs = match theirs_usd {
        Some(value) if value.is_finite() => value,
        Some(value) => {
            return Reconciliation::Unchecked {
                missing: format!("theirs_usd is {value}, which is not finite"),
            };
        }
        None => {
            return Reconciliation::Unchecked {
                missing: "theirs_usd is unavailable".to_string(),
            };
        }
    };

    let residual = theirs - ours;
    if residual == 0.0 {
        return Reconciliation::Agrees {
            ours_usd: ours,
            theirs_usd: theirs,
        };
    }
    let fraction = if theirs == 0.0 {
        f64::copysign(f64::MAX, residual)
    } else {
        residual / theirs
    };
    if fraction.abs() <= tolerance_fraction {
        Reconciliation::Agrees {
            ours_usd: ours,
            theirs_usd: theirs,
        }
    } else {
        Reconciliation::Differs {
            ours_usd: ours,
            theirs_usd: theirs,
            residual_usd: residual,
            fraction,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(
        arm: &str,
        usd: Measurement<f64>,
        billing: Billing,
        completed: bool,
        counts_for_arm: bool,
    ) -> RunCost {
        RunCost {
            arm: arm.to_string(),
            task: "task".to_string(),
            usd,
            billing,
            tokens: Measurement::observed(10),
            completed,
            counts_for_arm,
        }
    }

    fn measured_arm(name: &str, usd: f64, billing: Billing, completed: u32) -> ArmCost {
        ArmCost {
            arm: name.to_string(),
            runs: 1,
            completed,
            usd: Measurement::observed(usd),
            billing,
            tokens: Measurement::observed(10),
            unmeasured_runs: 0,
        }
    }

    #[test]
    fn subscription_zero_is_measured_but_unpriced_is_not() {
        let free = measured_arm("subscription", 0.0, Billing::Subscription, 1);
        let unpriced = measured_arm("unpriced", 0.0, Billing::MeteredUnpriced, 1);
        assert!(fully_measured(&free));
        assert!(!fully_measured(&unpriced));
    }

    #[test]
    fn missing_spend_reason_survives_cost_per_completion() {
        let reason = Absent::NotAttempted;
        let arm = ArmCost {
            arm: "unknown".to_string(),
            runs: 1,
            completed: 1,
            usd: Measurement::Missing(reason.clone()),
            billing: Billing::MeteredUnpriced,
            tokens: Measurement::not_attempted(),
            unmeasured_runs: 1,
        };
        assert_eq!(arm.usd_per_completion(), Measurement::Missing(reason));
    }

    #[test]
    fn missing_cost_is_excluded_from_frontier() {
        let arms = vec![
            measured_arm(
                "expensive",
                5.0,
                Billing::Metered {
                    input_per_mtok: 1.0,
                    output_per_mtok: 1.0,
                },
                1,
            ),
            ArmCost {
                arm: "unknown".to_string(),
                runs: 1,
                completed: 1,
                usd: Measurement::Missing(Absent::NotAttempted),
                billing: Billing::MeteredUnpriced,
                tokens: Measurement::not_attempted(),
                unmeasured_runs: 1,
            },
        ];
        assert_eq!(frontier(&arms, 0.0), vec!["expensive"]);
    }

    #[test]
    fn equal_cost_prefers_higher_completion_rate() {
        let low = measured_arm(
            "low-rate",
            2.0,
            Billing::Metered {
                input_per_mtok: 1.0,
                output_per_mtok: 1.0,
            },
            1,
        );
        let mut high = measured_arm(
            "high-rate",
            2.0,
            Billing::Metered {
                input_per_mtok: 1.0,
                output_per_mtok: 1.0,
            },
            2,
        );
        high.runs = 2;
        assert_eq!(frontier(&[low, high], 0.0), vec!["high-rate"]);
    }

    #[test]
    fn frontier_empty_and_without_measured_arms_is_empty() {
        assert!(frontier(&[], 0.0).is_empty());
        let unmeasured = ArmCost {
            arm: "unknown".to_string(),
            runs: 1,
            completed: 1,
            usd: Measurement::not_attempted(),
            billing: Billing::MeteredUnpriced,
            tokens: Measurement::not_attempted(),
            unmeasured_runs: 1,
        };
        assert!(frontier(&[unmeasured], 0.0).is_empty());
    }

    #[test]
    fn zero_completions_have_no_spend_per_completion() {
        let arm = measured_arm(
            "failed",
            2.0,
            Billing::Metered {
                input_per_mtok: 1.0,
                output_per_mtok: 1.0,
            },
            0,
        );
        assert!(matches!(
            arm.usd_per_completion(),
            Measurement::Missing(Absent::NothingToMeasure { .. })
        ));
    }

    #[test]
    fn aggregate_keeps_uncounted_arms_and_missing_totals() {
        let arms = aggregate(&[
            run(
                "z-arm",
                Measurement::observed(3.0),
                Billing::Metered {
                    input_per_mtok: 1.0,
                    output_per_mtok: 1.0,
                },
                true,
                false,
            ),
            run(
                "a-arm",
                Measurement::observed(2.0),
                Billing::Metered {
                    input_per_mtok: 1.0,
                    output_per_mtok: 1.0,
                },
                true,
                true,
            ),
            run(
                "a-arm",
                Measurement::not_attempted(),
                Billing::MeteredUnpriced,
                false,
                true,
            ),
        ]);
        assert_eq!(
            arms.iter().map(|arm| arm.arm.as_str()).collect::<Vec<_>>(),
            ["a-arm", "z-arm"]
        );
        assert_eq!(arms[0].runs, 2);
        assert_eq!(arms[0].completed, 1);
        assert!(matches!(arms[0].usd, Measurement::Missing(_)));
        assert_eq!(arms[1].runs, 1);
        assert_eq!(arms[1].completed, 0);
        assert!(matches!(arms[1].usd, Measurement::Missing(_)));
    }

    #[test]
    fn totals_never_present_a_partial_sum_as_observed() {
        let arms = vec![
            measured_arm(
                "known",
                2.0,
                Billing::Metered {
                    input_per_mtok: 1.0,
                    output_per_mtok: 1.0,
                },
                1,
            ),
            ArmCost {
                arm: "unknown".to_string(),
                runs: 1,
                completed: 0,
                usd: Measurement::Missing(Absent::NotAttempted),
                billing: Billing::MeteredUnpriced,
                tokens: Measurement::not_attempted(),
                unmeasured_runs: 1,
            },
        ];
        assert!(matches!(totals(&arms).0, Measurement::Missing(_)));
    }

    #[test]
    fn launcher_table_is_total_and_case_sensitive() {
        assert_eq!(launcher_from_name("agy"), Launcher::Agy);
        assert_eq!(launcher_from_name("claude"), Launcher::Claude);
        assert_eq!(launcher_from_name("gemini"), Launcher::Gemini);
        assert_eq!(launcher_from_name("ori"), Launcher::Ori);
        assert_eq!(launcher_from_name("AGY"), Launcher::Unknown);
    }

    #[test]
    fn token_dimensions_do_not_turn_missing_cache_into_zero() {
        let tokens = tokens_from_log(Launcher::Agy, "input tokens: 100\noutput tokens: 20\n");
        assert_eq!(tokens.input, Measurement::Observed(100));
        assert_eq!(tokens.output, Measurement::Observed(20));
        assert!(matches!(tokens.cache_read, Measurement::Missing(_)));
    }

    #[test]
    fn refusal_reset_is_observed_and_other_failures_are_missing() {
        assert_eq!(
            park_for(Launcher::Gemini, 429, "retry-after: 3600"),
            Measurement::Observed(3600)
        );
        assert!(matches!(
            park_for(Launcher::Gemini, 1, "connection reset by peer"),
            Measurement::Missing(_)
        ));
    }
}

#[cfg(test)]
mod token_scrape {
    use super::*;

    fn all_missing(tokens: &Tokens) -> bool {
        [&tokens.input, &tokens.output, &tokens.cache_read]
            .into_iter()
            .all(|measurement| matches!(measurement, Measurement::Missing(_)))
    }

    #[test]
    fn codex_comma_grouped_combined_report_is_not_assigned_to_a_dimension() {
        let tokens = tokens_from_log(Launcher::Codex, "tokens used\n130,826\n");
        assert!(all_missing(&tokens));
    }

    #[test]
    fn codex_bare_combined_report_is_not_assigned_to_a_dimension() {
        let tokens = tokens_from_log(Launcher::Codex, "tokens used\n4096");
        assert!(all_missing(&tokens));
    }

    #[test]
    fn codex_log_without_marker_has_no_dimensions() {
        assert!(all_missing(&tokens_from_log(
            Launcher::Codex,
            "stream closed\n"
        )));
    }

    #[test]
    fn codex_marker_as_final_line_has_no_dimensions() {
        for log in ["tokens used", "tokens used\n"] {
            assert!(all_missing(&tokens_from_log(Launcher::Codex, log)));
        }
    }

    #[test]
    fn prose_containing_marker_words_is_not_a_report() {
        let tokens = tokens_from_log(
            Launcher::Codex,
            "the tokens used by the arm were many\n130,826\n",
        );
        assert!(all_missing(&tokens));
    }

    #[test]
    fn marker_as_prefix_is_not_a_report() {
        assert!(all_missing(&tokens_from_log(
            Launcher::Codex,
            "tokens used: 130,826\n",
        )));
    }

    #[test]
    fn surrounding_marker_whitespace_is_accepted_but_stays_combined() {
        let tokens = tokens_from_log(Launcher::Codex, "  tokens used  \n130,826\n");
        assert!(all_missing(&tokens));
    }

    #[test]
    fn retry_reports_do_not_invent_a_dimension() {
        let tokens = tokens_from_log(Launcher::Codex, "tokens used\n100\ntokens used\n130,826\n");
        assert!(all_missing(&tokens));
    }

    #[test]
    fn count_on_a_later_line_is_not_a_report() {
        let tokens = tokens_from_log(Launcher::Codex, "tokens used\n(unavailable)\n130,826\n");
        assert!(all_missing(&tokens));
    }

    #[test]
    fn explicit_dimensions_keep_cache_missing_separate() {
        let tokens = tokens_from_log(Launcher::Codex, "input tokens: 100\noutput tokens: 20\n");
        assert_eq!(tokens.input, Measurement::Observed(100));
        assert_eq!(tokens.output, Measurement::Observed(20));
        assert!(matches!(tokens.cache_read, Measurement::Missing(_)));
    }

    #[test]
    fn labelled_count_uses_the_last_parseable_input_line() {
        let tokens = tokens_from_log(Launcher::Codex, "input: 100\ninput: 4096\n");
        assert_eq!(tokens.input, Measurement::Observed(4096));
    }

    #[test]
    fn an_unparseable_prefix_match_is_skipped_before_a_real_count() {
        let tokens = tokens_from_log(Launcher::Codex, "input path: /tmp/x\ninput: 42\n");
        assert_eq!(tokens.input, Measurement::Observed(42));
    }

    #[test]
    fn a_label_without_any_parseable_count_remains_missing() {
        let tokens = tokens_from_log(Launcher::Codex, "input path: /tmp/x\ninput: n/a\n");
        assert!(matches!(tokens.input, Measurement::Missing(_)));
    }

    #[test]
    fn unknown_launcher_does_not_interpret_a_codex_marker() {
        assert!(all_missing(&tokens_from_log(
            Launcher::Unknown,
            "tokens used\n130,826\n",
        )));
    }
}

#[cfg(test)]
mod token_boundaries {
    use super::*;

    #[test]
    fn reported_zero_is_observed_zero() {
        let tokens = tokens_from_log(Launcher::Codex, "input tokens: 0\n");
        assert_eq!(tokens.input, Measurement::Observed(0));
    }

    #[test]
    fn empty_log_is_missing() {
        let tokens = tokens_from_log(Launcher::Codex, "");
        assert!(matches!(tokens.input, Measurement::Missing(_)));
        assert!(matches!(tokens.output, Measurement::Missing(_)));
        assert!(matches!(tokens.cache_read, Measurement::Missing(_)));
    }

    #[test]
    fn u64_max_parses_with_comma_grouping() {
        let tokens = tokens_from_log(
            Launcher::Codex,
            "input tokens: 18,446,744,073,709,551,615\n",
        );
        assert_eq!(tokens.input, Measurement::Observed(u64::MAX));
    }

    #[test]
    fn overflowing_count_is_missing_not_wrapped() {
        let tokens = tokens_from_log(
            Launcher::Codex,
            "input tokens: 99,999,999,999,999,999,999\n",
        );
        assert!(matches!(tokens.input, Measurement::Missing(_)));
    }

    #[test]
    fn stray_whitespace_around_count_parses() {
        let tokens = tokens_from_log(Launcher::Codex, "input tokens: \t130,826  \n");
        assert_eq!(tokens.input, Measurement::Observed(130_826));
    }

    #[test]
    fn non_numeric_payload_is_missing() {
        let tokens = tokens_from_log(Launcher::Codex, "input tokens: n/a\n");
        assert!(matches!(tokens.input, Measurement::Missing(_)));
    }

    #[test]
    fn negative_payload_is_missing() {
        let tokens = tokens_from_log(Launcher::Codex, "input tokens: -5\n");
        assert!(matches!(tokens.input, Measurement::Missing(_)));
    }
}

#[cfg(test)]
mod agy_opencode_missing {
    use super::*;

    fn reason_of(measurement: &Measurement<u64>) -> String {
        match measurement {
            Measurement::Missing(Absent::NothingToMeasure { reason })
            | Measurement::Missing(Absent::InstrumentFailed { reason })
            | Measurement::Missing(Absent::Untrusted { reason }) => reason.clone(),
            Measurement::Missing(Absent::NotAttempted) => "not attempted".to_string(),
            Measurement::Observed(_) => "observed".to_string(),
        }
    }

    #[test]
    fn agy_is_missing_on_logs_without_dimensions() {
        for log in ["step 1 done\n", "", "tokens used\n130,826\n"] {
            let tokens = tokens_from_log(Launcher::Agy, log);
            for measurement in [&tokens.input, &tokens.output, &tokens.cache_read] {
                assert!(matches!(measurement, Measurement::Missing(_)));
            }
        }
    }

    #[test]
    fn opencode_is_missing_on_logs_without_dimensions() {
        for log in ["step 1 done\n", "", "tokens used\n130,826\n"] {
            let tokens = tokens_from_log(Launcher::Opencode, log);
            for measurement in [&tokens.input, &tokens.output, &tokens.cache_read] {
                assert!(matches!(measurement, Measurement::Missing(_)));
            }
        }
    }

    #[test]
    #[ignore = "existing implementation does not distinguish these launcher reasons"]
    fn agy_and_opencode_reasons_are_different_facts() {
        let agy = tokens_from_log(Launcher::Agy, "");
        let opencode = tokens_from_log(Launcher::Opencode, "");
        assert_ne!(reason_of(&agy.input), reason_of(&opencode.input));
    }

    #[test]
    #[ignore = "existing implementation does not include launcher names in these reasons"]
    fn known_launcher_reasons_name_the_launcher() {
        let agy = tokens_from_log(Launcher::Agy, "");
        let opencode = tokens_from_log(Launcher::Opencode, "");
        assert!(reason_of(&agy.input).to_lowercase().contains("agy"));
        assert!(
            reason_of(&opencode.input)
                .to_lowercase()
                .contains("opencode")
        );
    }

    #[test]
    fn missing_launcher_dimensions_do_not_read_as_zero() {
        let tokens = tokens_from_log(Launcher::Agy, "tokens used\n0\n");
        assert!(matches!(tokens.input, Measurement::Missing(_)));
        assert!(matches!(tokens.output, Measurement::Missing(_)));
        assert!(matches!(tokens.cache_read, Measurement::Missing(_)));
    }
}

#[cfg(test)]
mod reconciliation {
    use super::*;

    fn missing(result: Reconciliation) -> String {
        match result {
            Reconciliation::Unchecked { missing } => missing,
            other => panic!("expected Unchecked, got {other:?}"),
        }
    }

    #[test]
    fn residual_is_theirs_minus_ours_and_fraction_uses_theirs() {
        match reconcile(Some(16.8747), Some(20.0299), 0.01) {
            Reconciliation::Differs {
                residual_usd,
                fraction,
                ..
            } => {
                assert!((residual_usd - 3.1552).abs() < 1e-9);
                assert!((fraction - 0.1575).abs() < 1e-4);
            }
            other => panic!("expected Differs, got {other:?}"),
        }
    }

    #[test]
    fn half_percent_is_inside_one_percent() {
        assert!(matches!(
            reconcile(Some(20.0), Some(20.1), 0.01),
            Reconciliation::Agrees { .. }
        ));
    }

    #[test]
    fn missing_provider_is_unchecked() {
        assert!(matches!(
            reconcile(Some(20.0), None, 0.01),
            Reconciliation::Unchecked { .. }
        ));
    }

    #[test]
    fn missing_sides_have_distinct_reasons() {
        assert_ne!(
            missing(reconcile(None, Some(20.0), 0.01)),
            missing(reconcile(Some(20.0), None, 0.01))
        );
    }

    #[test]
    fn both_sides_missing_is_unchecked() {
        assert!(matches!(
            reconcile(None, None, 0.01),
            Reconciliation::Unchecked { .. }
        ));
    }

    #[test]
    fn overcount_has_negative_residual() {
        match reconcile(Some(21.0), Some(20.0), 0.01) {
            Reconciliation::Differs { residual_usd, .. } => assert_eq!(residual_usd, -1.0),
            other => panic!("expected Differs, got {other:?}"),
        }
    }

    #[test]
    fn tolerance_boundary_is_inclusive() {
        assert!(matches!(
            reconcile(Some(99.0), Some(100.0), 0.01),
            Reconciliation::Agrees { .. }
        ));
    }

    #[test]
    fn just_inside_tolerance_agrees() {
        assert!(matches!(
            reconcile(Some(99.1), Some(100.0), 0.01),
            Reconciliation::Agrees { .. }
        ));
    }

    #[test]
    fn just_outside_tolerance_differs() {
        assert!(matches!(
            reconcile(Some(98.9), Some(100.0), 0.01),
            Reconciliation::Differs { .. }
        ));
    }

    #[test]
    fn both_zero_agree() {
        assert!(matches!(
            reconcile(Some(0.0), Some(0.0), 0.01),
            Reconciliation::Agrees { .. }
        ));
    }

    #[test]
    fn zero_provider_has_finite_fraction() {
        match reconcile(Some(5.0), Some(0.0), 0.01) {
            Reconciliation::Differs {
                residual_usd,
                fraction,
                ..
            } => {
                assert_eq!(residual_usd, -5.0);
                assert!(fraction.is_finite());
            }
            other => panic!("expected Differs, got {other:?}"),
        }
    }

    #[test]
    fn zero_tolerance_only_agrees_on_identical_values() {
        assert!(matches!(
            reconcile(Some(20.0), Some(20.0), 0.0),
            Reconciliation::Agrees { .. }
        ));
        assert!(matches!(
            reconcile(Some(20.0), Some(20.0 + 1e-9), 0.0),
            Reconciliation::Differs { .. }
        ));
    }

    #[test]
    fn negative_tolerance_is_unchecked() {
        assert!(matches!(
            reconcile(Some(1.0), Some(1.0), -0.01),
            Reconciliation::Unchecked { .. }
        ));
    }

    #[test]
    fn refunds_are_signed() {
        assert!(matches!(
            reconcile(Some(-1.0), Some(-1.0), 0.01),
            Reconciliation::Agrees { .. }
        ));
        match reconcile(Some(-16.8747), Some(-20.0299), 0.01) {
            Reconciliation::Differs { residual_usd, .. } => {
                assert!((residual_usd + 3.1552).abs() < 1e-9)
            }
            other => panic!("expected Differs, got {other:?}"),
        }
    }

    #[test]
    fn non_finite_figures_are_unchecked() {
        for (ours, theirs) in [
            (Some(f64::NAN), Some(20.0)),
            (Some(f64::INFINITY), Some(20.0)),
            (Some(20.0), Some(f64::NAN)),
            (Some(20.0), Some(f64::NEG_INFINITY)),
        ] {
            assert!(matches!(
                reconcile(ours, theirs, 0.01),
                Reconciliation::Unchecked { .. }
            ));
        }
    }

    #[test]
    fn finite_equal_values_agree() {
        assert!(matches!(
            reconcile(Some(7.5), Some(7.5), 0.0),
            Reconciliation::Agrees { .. }
        ));
    }

    #[test]
    fn non_finite_tolerance_is_unchecked() {
        assert!(matches!(
            reconcile(Some(1.0), Some(1.0), f64::NAN),
            Reconciliation::Unchecked { .. }
        ));
    }
}
