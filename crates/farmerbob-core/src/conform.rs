//! Classification and aggregation for a grafted conformance suite.
//!
//! A suite that did not compile, or did not execute exactly the tests it
//! declared, cannot certify an arm. The count comparison stays separate from
//! the failure count so that a silent graft mismatch is not mistaken for a
//! passing or failing suite.

use crate::measurement::Measurement;

/// What running a grafted conformance suite against one arm produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    /// Tests the suite file declares, counted before grafting.
    pub expected: u32,
    /// Tests that actually executed, from the test log.
    pub executed: u32,
    /// Tests that failed.
    pub failed: u32,
}

/// One arm's conformance result. Exactly these and no others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conformance {
    /// Every expected test ran and none failed.
    Full,
    /// Tests ran and at least one failed.
    Broke {
        /// How many failed.
        failed: u32,
    },
    /// A different number of tests executed than the suite declares. The
    /// suite did not run as written and the result says nothing about the arm.
    Short {
        /// How many were expected.
        expected: u32,
        /// How many executed.
        executed: u32,
    },
    /// The graft did not compile, or the arm had no crate to graft onto.
    NotGrafted,
}

/// Read one arm's outcome.
///
/// `compiled` is whether the grafted copy built at all. A compiled suite must
/// execute a non-zero, exact number of tests before its failure count can
/// classify the arm.
pub fn read(compiled: bool, ran: &Ran) -> Conformance {
    if !compiled {
        return Conformance::NotGrafted;
    }

    if ran.expected == 0 || ran.executed != ran.expected {
        return Conformance::Short {
            expected: ran.expected,
            executed: ran.executed,
        };
    }

    if ran.failed > 0 {
        Conformance::Broke { failed: ran.failed }
    } else {
        Conformance::Full
    }
}

/// Return the fraction of graftable arms that reached [`Conformance::Full`].
///
/// [`Conformance::NotGrafted`] arms are excluded from the denominator. When
/// no arm is graftable, the result is [`Measurement::Missing`] because there
/// is no field observation to measure.
pub fn rate(results: &[Conformance]) -> Measurement<f64> {
    let graftable = results
        .iter()
        .filter(|result| !matches!(result, Conformance::NotGrafted))
        .count();

    if graftable == 0 {
        return Measurement::nothing_to_measure("no arm was graftable");
    }

    let full = results
        .iter()
        .filter(|result| matches!(result, Conformance::Full))
        .count();
    Measurement::observed(full as f64 / graftable as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measurement::Absent;

    #[test]
    fn read_rejects_an_uncompiled_graft_regardless_of_counts() {
        let ran = Ran {
            expected: 3,
            executed: 1,
            failed: 1,
        };

        assert_eq!(read(false, &ran), Conformance::NotGrafted);
    }

    #[test]
    fn read_requires_a_nonzero_exact_run_before_classifying_failures() {
        let cases = [
            (
                Ran {
                    expected: 2,
                    executed: 2,
                    failed: 0,
                },
                Conformance::Full,
            ),
            (
                Ran {
                    expected: 2,
                    executed: 2,
                    failed: 1,
                },
                Conformance::Broke { failed: 1 },
            ),
            (
                Ran {
                    expected: 3,
                    executed: 2,
                    failed: 0,
                },
                Conformance::Short {
                    expected: 3,
                    executed: 2,
                },
            ),
            (
                Ran {
                    expected: 2,
                    executed: 3,
                    failed: 0,
                },
                Conformance::Short {
                    expected: 2,
                    executed: 3,
                },
            ),
            (
                Ran {
                    expected: 0,
                    executed: 0,
                    failed: 0,
                },
                Conformance::Short {
                    expected: 0,
                    executed: 0,
                },
            ),
        ];

        for (ran, expected) in cases {
            assert_eq!(read(true, &ran), expected);
        }
    }

    #[test]
    fn rate_is_missing_when_there_are_no_results() {
        match rate(&[]) {
            Measurement::Missing(Absent::NothingToMeasure { reason }) => {
                assert!(!reason.is_empty());
            }
            other => panic!("unexpected rate: {other:?}"),
        }
    }

    #[test]
    fn rate_is_missing_when_every_arm_is_not_grafted() {
        match rate(&[Conformance::NotGrafted, Conformance::NotGrafted]) {
            Measurement::Missing(Absent::NothingToMeasure { reason }) => {
                assert!(!reason.is_empty());
            }
            other => panic!("unexpected rate: {other:?}"),
        }
    }

    #[test]
    fn rate_counts_only_graftable_arms_and_includes_short_and_broke() {
        let results = [
            Conformance::Full,
            Conformance::Broke { failed: 1 },
            Conformance::Short {
                expected: 2,
                executed: 1,
            },
            Conformance::NotGrafted,
        ];

        assert_eq!(rate(&results), Measurement::Observed(1.0 / 3.0));
    }

    #[test]
    fn rate_pins_zero_and_one_observed_boundaries() {
        assert_eq!(
            rate(&[Conformance::Full, Conformance::Full]),
            Measurement::Observed(1.0)
        );
        assert_eq!(
            rate(&[
                Conformance::Broke { failed: 1 },
                Conformance::Broke { failed: 2 },
            ]),
            Measurement::Observed(0.0)
        );
        assert_eq!(
            rate(&[Conformance::Full, Conformance::NotGrafted]),
            Measurement::Observed(1.0)
        );
    }

    #[test]
    fn a_short_run_and_an_ungraftable_arm_are_distinct_in_rate() {
        let short = Ran {
            expected: 2,
            executed: 1,
            failed: 0,
        };
        let results = [read(true, &short), read(false, &short)];

        assert_eq!(
            results[0],
            Conformance::Short {
                expected: 2,
                executed: 1,
            }
        );
        assert_eq!(results[1], Conformance::NotGrafted);
        assert_eq!(rate(&results), Measurement::Observed(0.0));
    }
}
