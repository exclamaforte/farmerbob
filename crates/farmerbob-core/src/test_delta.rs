//! Compare the crate's measured test counts before and after a candidate run.

use crate::measurement::Measurement;

/// Test counts around one candidate's run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Counts {
    /// Tests passing in the crate before the run, on the base.
    /// `Missing` means the baseline could not be measured.
    pub before: Measurement<u32>,
    /// Tests passing in the crate after the run.
    /// `Missing` means the run's own suite could not be read.
    pub after: Measurement<u32>,
}

/// What a candidate contributed to the crate's suite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Contribution {
    /// The candidate added this many passing tests.
    Added(u32),
    /// The crate's passing count went down by this many.
    Removed(u32),
    /// The measured count did not change.
    Unchanged,
}

/// What the candidate contributed, or why that cannot be said.
pub fn contribution(c: &Counts) -> Measurement<Contribution> {
    match (&c.before, &c.after) {
        (Measurement::Missing(absent), _) => Measurement::Missing(absent.clone()),
        (_, Measurement::Missing(absent)) => Measurement::Missing(absent.clone()),
        (Measurement::Observed(before), Measurement::Observed(after)) => {
            if after > before {
                Measurement::Observed(Contribution::Added(after - before))
            } else if before > after {
                Measurement::Observed(Contribution::Removed(before - after))
            } else {
                Measurement::Observed(Contribution::Unchanged)
            }
        }
    }
}

/// Return the crate-wide count after the run, unchanged.
pub fn crate_total(c: &Counts) -> Measurement<u32> {
    c.after.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measurement::Absent;

    #[test]
    fn reports_added_removed_and_unchanged_counts() {
        let cases = [
            (
                Counts {
                    before: Measurement::Observed(541),
                    after: Measurement::Observed(547),
                },
                Measurement::Observed(Contribution::Added(6)),
            ),
            (
                Counts {
                    before: Measurement::Observed(541),
                    after: Measurement::Observed(541),
                },
                Measurement::Observed(Contribution::Unchanged),
            ),
            (
                Counts {
                    before: Measurement::Observed(547),
                    after: Measurement::Observed(541),
                },
                Measurement::Observed(Contribution::Removed(6)),
            ),
        ];

        for (counts, expected) in cases {
            assert_eq!(contribution(&counts), expected);
        }
    }

    #[test]
    fn missing_baseline_is_not_treated_as_zero() {
        let counts = Counts {
            before: Measurement::Missing(Absent::NotAttempted),
            after: Measurement::Observed(547),
        };

        assert_eq!(
            contribution(&counts),
            Measurement::Missing(Absent::NotAttempted)
        );
        assert_eq!(crate_total(&counts), Measurement::Observed(547));
    }

    #[test]
    fn missing_after_is_propagated_by_crate_total_and_contribution() {
        let absent = Absent::InstrumentFailed {
            reason: "suite unavailable".to_string(),
        };
        let counts = Counts {
            before: Measurement::Observed(541),
            after: Measurement::Missing(absent.clone()),
        };

        assert_eq!(contribution(&counts), Measurement::Missing(absent.clone()));
        assert_eq!(crate_total(&counts), Measurement::Missing(absent));
    }

    #[test]
    fn zero_and_maximum_counts_do_not_overflow() {
        assert_eq!(
            contribution(&Counts {
                before: Measurement::Observed(0),
                after: Measurement::Observed(0),
            }),
            Measurement::Observed(Contribution::Unchanged)
        );
        assert_eq!(
            contribution(&Counts {
                before: Measurement::Observed(0),
                after: Measurement::Observed(u32::MAX),
            }),
            Measurement::Observed(Contribution::Added(u32::MAX))
        );
        assert_eq!(
            contribution(&Counts {
                before: Measurement::Observed(u32::MAX),
                after: Measurement::Observed(0),
            }),
            Measurement::Observed(Contribution::Removed(u32::MAX))
        );
    }
}
