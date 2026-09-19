//! Decide whether observed evidence clears a task's gate, per verification kind.
//!
//! Each kind of task has its own instrument: deterministic tests are read by
//! [`crate::build_verdict`], benchmarks are scored by
//! [`crate::task_contract::score`], and reviewer work arrives as a written
//! judgement. This module is the branch on
//! [`crate::task_contract::Verification`] that turns an instrument's
//! already-decided output into one [`Cleared`] verdict. It runs nothing — no
//! cargo, no benchmarks, no files: evidence arrives final, and the only
//! question here is whether it clears the gate for the kind that was declared.

use crate::build_verdict::BuildVerdict;
use crate::task_contract::{Score, Verification};

/// What the harness observed for a task, in whichever form its kind produces.
#[derive(Debug, Clone, PartialEq)]
pub enum Evidence {
    /// A cargo run: the build-and-test verdict `build_verdict` already decides.
    Tests(BuildVerdict),
    /// A benchmark run: the score `task_contract::score` already decides.
    Bench(Score),
    /// A reviewer's judgement. Carries whether it passed and a non-empty grounds.
    Review {
        /// Whether the reviewer judged the task cleared.
        passed: bool,
        /// The reviewer's stated grounds.
        grounds: String,
    },
}

/// Whether a candidate cleared its task's gate.
#[derive(Debug, Clone, PartialEq)]
pub enum Cleared {
    /// It cleared. Carries a one-line summary for an operator.
    Yes(String),
    /// It did not. Carries a one-line reason.
    No(String),
    /// The evidence does not match the task's declared verification kind, so no
    /// verdict is possible. Carries what was declared and what arrived.
    Mismatched {
        /// The verification kind the task declared.
        declared: Verification,
        /// What arrived instead, named for the caller's diagnostics.
        got: String,
    },
}

/// Decide whether the evidence clears the gate for this verification kind.
///
/// Evidence of the wrong variant is [`Cleared::Mismatched`], never
/// [`Cleared::No`]: a mismatch is a fault in the harness, not a failure of the
/// candidate, and the two must stay distinguishable.
pub fn cleared(kind: Verification, e: &Evidence) -> Cleared {
    match (kind, e) {
        (Verification::DeterministicTests, Evidence::Tests(verdict)) => tests_verdict(verdict),
        (Verification::Benchmark, Evidence::Bench(score)) => bench_score(score),
        (Verification::ReviewerJudgement, Evidence::Review { passed, grounds }) => {
            review(*passed, grounds)
        }
        (declared, other) => Cleared::Mismatched {
            declared,
            got: describe(other),
        },
    }
}

/// The evidence variant a kind requires, as a name, for a caller's diagnostics.
pub fn expects(kind: Verification) -> &'static str {
    match kind {
        Verification::DeterministicTests => "Tests(BuildVerdict)",
        Verification::Benchmark => "Bench(Score)",
        Verification::ReviewerJudgement => "Review { passed, grounds }",
    }
}

/// Read an already-decided build-and-test verdict.
fn tests_verdict(verdict: &BuildVerdict) -> Cleared {
    match verdict {
        BuildVerdict::Passed { passed } => Cleared::Yes(format!("cleared: {passed} tests passed")),
        BuildVerdict::Failed { failed } => {
            Cleared::No(format!("not cleared: {failed} tests failed"))
        }
        BuildVerdict::NoTests => Cleared::No(
            "not cleared: no test executed, and the gate requires at least one".to_string(),
        ),
        BuildVerdict::BuildFailed => Cleared::No("not cleared: the build failed".to_string()),
    }
}

/// Read an already-decided benchmark score.
fn bench_score(score: &Score) -> Cleared {
    match score {
        Score::Scored { speedup, trials } => Cleared::Yes(format!(
            "cleared benchmark: speedup {speedup} over {trials} trials"
        )),
        Score::Incorrect { detail } => {
            Cleared::No(format!("not cleared: the run was incorrect: {detail}"))
        }
        Score::Unreliable { reason } => Cleared::No(format!(
            "not cleared: the benchmark measurement is unreliable: {reason}"
        )),
        Score::NotMeasured { reason } => Cleared::No(format!(
            "not cleared: the benchmark was not measured: {reason}"
        )),
    }
}

/// Read an already-made reviewer judgement.
fn review(passed: bool, grounds: &str) -> Cleared {
    if passed {
        Cleared::Yes("cleared: the reviewer approved".to_string())
    } else if grounds.is_empty() {
        Cleared::No("not cleared: the reviewer rejected without stating grounds".to_string())
    } else {
        Cleared::No(format!("not cleared: the reviewer rejected: {grounds}"))
    }
}

/// Name the evidence that arrived, for a `Mismatched` report.
fn describe(e: &Evidence) -> String {
    match e {
        Evidence::Tests(_) => "a build-and-test verdict (Evidence::Tests)".to_string(),
        Evidence::Bench(_) => "a benchmark score (Evidence::Bench)".to_string(),
        Evidence::Review { .. } => "a reviewer judgement (Evidence::Review)".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Clause 1: a passed build-and-test run clears `DeterministicTests`.
    #[test]
    fn passed_tests_clear_the_deterministic_gate() {
        let e = Evidence::Tests(BuildVerdict::Passed { passed: 3 });
        assert!(matches!(
            cleared(Verification::DeterministicTests, &e),
            Cleared::Yes(_)
        ));
    }

    /// Clauses 2 and 6 from one kind, as the spec requires: every failing
    /// verdict is `No` with a non-empty reason, while the wrong instruments
    /// are `Mismatched` carrying the declared kind. An implementation that
    /// answers `No` to everything it cannot clear fails the second half.
    #[test]
    fn deterministic_tests_no_and_mismatched_from_the_same_kind() {
        let failures = [
            Evidence::Tests(BuildVerdict::BuildFailed),
            Evidence::Tests(BuildVerdict::Failed { failed: 2 }),
            Evidence::Tests(BuildVerdict::NoTests),
        ];
        for e in &failures {
            match cleared(Verification::DeterministicTests, e) {
                Cleared::No(reason) => assert!(!reason.is_empty()),
                other => panic!("expected No, got {other:?}"),
            }
        }

        let wrong_instruments = [
            Evidence::Bench(Score::Scored {
                speedup: 1.0,
                trials: 1,
            }),
            Evidence::Review {
                passed: true,
                grounds: "fine".to_string(),
            },
        ];
        for e in &wrong_instruments {
            match cleared(Verification::DeterministicTests, e) {
                Cleared::Mismatched { declared, .. } => {
                    assert_eq!(declared, Verification::DeterministicTests)
                }
                other => panic!("expected Mismatched, got {other:?}"),
            }
        }
    }

    /// Clause 6 across the remaining four wrong pairings: whichever kind is
    /// handed another instrument's output, the verdict is `Mismatched` naming
    /// that kind, never `No`.
    #[test]
    fn remaining_wrong_pairings_are_mismatched() {
        let wrong = [
            (
                Verification::Benchmark,
                Evidence::Tests(BuildVerdict::Passed { passed: 1 }),
            ),
            (
                Verification::Benchmark,
                Evidence::Review {
                    passed: false,
                    grounds: String::new(),
                },
            ),
            (
                Verification::ReviewerJudgement,
                Evidence::Tests(BuildVerdict::NoTests),
            ),
            (
                Verification::ReviewerJudgement,
                Evidence::Bench(Score::Incorrect {
                    detail: "x".to_string(),
                }),
            ),
        ];
        for (kind, e) in &wrong {
            match cleared(*kind, e) {
                Cleared::Mismatched { declared, .. } => assert_eq!(declared, *kind),
                other => panic!("expected Mismatched, got {other:?}"),
            }
        }
    }

    /// Clause 3 and the zero boundary: a `Scored` benchmark clears, and the
    /// summary carries the speedup number itself; a measured zero speedup is
    /// still `Yes`, because whether a slow kernel passes is
    /// `task_contract::score`'s decision, already made.
    #[test]
    fn scored_benchmarks_clear_and_name_the_speedup() {
        let e = Evidence::Bench(Score::Scored {
            speedup: 2.5,
            trials: 4,
        });
        match cleared(Verification::Benchmark, &e) {
            Cleared::Yes(summary) => assert!(summary.contains("2.5")),
            other => panic!("expected Yes, got {other:?}"),
        }

        let zero = Evidence::Bench(Score::Scored {
            speedup: 0.0,
            trials: 1,
        });
        assert!(matches!(
            cleared(Verification::Benchmark, &zero),
            Cleared::Yes(_)
        ));
    }

    /// Clause 4: an `Incorrect` score is `No`, and the reason mentions
    /// correctness — checked case-insensitively, never by exact wording.
    #[test]
    fn incorrect_benchmarks_reject_mentioning_correctness() {
        let e = Evidence::Bench(Score::Incorrect {
            detail: "output diverged from the reference".to_string(),
        });
        match cleared(Verification::Benchmark, &e) {
            Cleared::No(reason) => {
                assert!(!reason.is_empty());
                assert!(reason.to_lowercase().contains("correct"));
            }
            other => panic!("expected No, got {other:?}"),
        }
    }

    /// Clause 5 and the review boundaries: a passing review clears, a
    /// rejection is `No` carrying the grounds it was given; empty grounds on
    /// a pass still clears, and empty grounds on a rejection still yield a
    /// non-empty reason.
    #[test]
    fn reviewer_judgements_follow_the_review() {
        let yes = Evidence::Review {
            passed: true,
            grounds: "checked against the reference".to_string(),
        };
        assert!(matches!(
            cleared(Verification::ReviewerJudgement, &yes),
            Cleared::Yes(_)
        ));

        let no = Evidence::Review {
            passed: false,
            grounds: "kernel diverges on the edge cases".to_string(),
        };
        match cleared(Verification::ReviewerJudgement, &no) {
            Cleared::No(reason) => assert!(reason.contains("kernel diverges on the edge cases")),
            other => panic!("expected No, got {other:?}"),
        }

        let bare_yes = Evidence::Review {
            passed: true,
            grounds: String::new(),
        };
        assert!(matches!(
            cleared(Verification::ReviewerJudgement, &bare_yes),
            Cleared::Yes(_)
        ));

        let bare_no = Evidence::Review {
            passed: false,
            grounds: String::new(),
        };
        match cleared(Verification::ReviewerJudgement, &bare_no) {
            Cleared::No(reason) => assert!(!reason.is_empty()),
            other => panic!("expected No, got {other:?}"),
        }
    }

    /// Clause 8: every kind expects something, and no two expectations share
    /// a name. Combined with the tests above — right variant never
    /// `Mismatched`, wrong variant always `Mismatched`, across all three
    /// kinds — `expects` and `cleared` agree without this suite asserting
    /// the names themselves.
    #[test]
    fn expectations_are_non_empty_and_distinct() {
        let names = [
            expects(Verification::DeterministicTests),
            expects(Verification::Benchmark),
            expects(Verification::ReviewerJudgement),
        ];
        for name in names {
            assert!(!name.is_empty());
        }
        assert_ne!(names[0], names[1]);
        assert_ne!(names[1], names[2]);
        assert_ne!(names[0], names[2]);
    }
}
