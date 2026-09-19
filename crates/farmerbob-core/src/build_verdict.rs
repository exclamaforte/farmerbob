//! Turn build and test log outcomes into a small, explicit verdict.

use crate::measurement::Measurement;

const RESULT_PREFIX: &str = "test result:";

/// What a build-and-test pair says about a run. Exactly these and no others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildVerdict {
    /// Built, and at least one test ran and passed.
    Passed {
        /// How many tests passed, summed across targets.
        passed: u32,
    },
    /// Built, tests ran, at least one failed.
    Failed {
        /// How many failed, summed across targets.
        failed: u32,
    },
    /// Built, and no test executed. Never `Passed`.
    NoTests,
    /// Did not build.
    BuildFailed,
}

/// Read a build log and a test log.
///
/// `built` is whether the build command exited 0; the log is read only for
/// the counts.
pub fn read(built: bool, test_log: &str) -> BuildVerdict {
    if !built {
        return BuildVerdict::BuildFailed;
    }

    let (passed, failed) = totals(test_log);
    if failed > 0 {
        BuildVerdict::Failed { failed }
    } else if passed > 0 {
        BuildVerdict::Passed { passed }
    } else {
        BuildVerdict::NoTests
    }
}

/// How many tests ran, or `Missing` when the log carries no result line at
/// all—which is not the same as zero.
pub fn tests_run(test_log: &str) -> Measurement<u32> {
    let mut found_result = false;
    let mut passed: u32 = 0;

    for line in test_log.lines() {
        if let Some((line_passed, _)) = result_counts(line) {
            found_result = true;
            passed = passed.saturating_add(line_passed);
        }
    }

    if found_result {
        Measurement::Observed(passed)
    } else {
        Measurement::Missing(crate::measurement::Absent::NothingToMeasure {
            reason: "test log has no result line".to_string(),
        })
    }
}

fn totals(test_log: &str) -> (u32, u32) {
    test_log.lines().filter_map(result_counts).fold(
        (0, 0),
        |(passed, failed), (line_passed, line_failed)| {
            (
                passed.saturating_add(line_passed),
                failed.saturating_add(line_failed),
            )
        },
    )
}

fn result_counts(line: &str) -> Option<(u32, u32)> {
    let remainder = line.strip_prefix(RESULT_PREFIX)?;
    let mut passed: Option<u32> = None;
    let mut failed: Option<u32> = None;

    let mut previous: Option<&str> = None;
    for raw_word in remainder.split_whitespace() {
        let word = raw_word.trim_end_matches(';');
        if word == "passed" {
            passed = previous.and_then(|count| count.parse().ok());
        } else if word == "failed" {
            failed = previous.and_then(|count| count.parse().ok());
        }
        previous = Some(word);
    }

    Some((passed.unwrap_or(0), failed.unwrap_or(0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_failure_wins_over_a_clean_test_log() {
        let log = "test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out";

        assert_eq!(read(false, log), BuildVerdict::BuildFailed);
    }

    #[test]
    fn passing_results_are_summed_across_targets() {
        let log = concat!(
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n",
            "test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"
        );

        assert_eq!(read(true, log), BuildVerdict::Passed { passed: 3 });
        assert_eq!(tests_run(log), Measurement::Observed(3));
    }

    #[test]
    fn any_failure_wins_and_failures_are_summed() {
        let log = concat!(
            "test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n",
            "test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n",
            "test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out"
        );

        assert_eq!(read(true, log), BuildVerdict::Failed { failed: 3 });
    }

    #[test]
    fn zero_result_and_missing_result_are_different_measurements() {
        // Clauses 4 and 7 must be checked together: the verdict must reject
        // an empty suite while the count must still report an observed zero.
        let zero = "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out";

        assert_eq!(read(true, zero), BuildVerdict::NoTests);
        assert_eq!(tests_run(zero), Measurement::Observed(0));
        assert_eq!(read(true, ""), BuildVerdict::NoTests);
        assert!(matches!(tests_run(""), Measurement::Missing(_)));
    }

    #[test]
    fn one_pass_and_one_failure_keep_their_boundary_behaviour() {
        let one_pass = "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out";
        let one_failure =
            "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out";

        assert_eq!(read(true, one_pass), BuildVerdict::Passed { passed: 1 });
        assert_eq!(read(true, one_failure), BuildVerdict::Failed { failed: 1 });
    }

    #[test]
    fn result_text_in_output_is_not_a_result_line() {
        let log = concat!(
            "test example ... ok\n",
            "stdout: test result: ok. 99 passed; 0 failed; prose only\n",
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"
        );

        assert_eq!(read(true, log), BuildVerdict::Passed { passed: 1 });
        assert_eq!(tests_run(log), Measurement::Observed(1));
    }

    #[test]
    fn verdict_and_count_agree_on_four_kinds_of_log() {
        let logs = [
            (
                "test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out",
                BuildVerdict::Passed { passed: 2 },
            ),
            (
                "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out",
                BuildVerdict::NoTests,
            ),
            ("", BuildVerdict::NoTests),
            (
                "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out",
                BuildVerdict::Failed { failed: 1 },
            ),
        ];

        for (log, verdict) in logs {
            let count = tests_run(log);
            match (read(true, log), count) {
                (BuildVerdict::Passed { .. }, Measurement::Observed(n)) => assert!(n > 0),
                (BuildVerdict::NoTests, Measurement::Observed(n)) => assert_eq!(n, 0),
                (BuildVerdict::NoTests, Measurement::Missing(_)) => {}
                (BuildVerdict::Failed { .. }, _) => {}
                (actual, _) => panic!("unexpected aggregate: {actual:?}, expected {verdict:?}"),
            }
            assert_eq!(read(true, log), verdict);
        }
    }
}
