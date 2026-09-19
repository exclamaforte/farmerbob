//! Run timing derivation from file modification times.
//!
//! Reconstructs time-to-first-write, production span, and throughput (lines per
//! minute) from observed file mtimes. Pure domain logic with no I/O or system clock.

use crate::measurement::Measurement;

/// One changed file and when it was last written, as the caller stat-ed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wrote {
    /// Repo-relative path.
    pub path: String,
    /// Modification time, seconds since the epoch.
    pub mtime_s: i64,
}

/// What a run's file timestamps say about how it spent its time.
#[derive(Debug, Clone, PartialEq)]
pub struct RunTiming {
    /// Seconds from the run starting to its FIRST write.
    pub ttfw_s: i64,
    /// Seconds from the first write to the last.
    pub span_s: i64,
    /// Lines added across those files, as the caller counted them.
    pub lines: u32,
    /// Lines per minute over `span_s`, or `Missing` when it cannot be a
    /// rate -- see the boundaries.
    pub lines_per_min: Measurement<f64>,
}

impl Eq for RunTiming {}

/// Derive the three numbers. `started_s` is when the run began.
///
/// `Missing` when `wrote` is empty: a run that wrote nothing has no
/// first write, and reporting zero would say it wrote instantly.
pub fn derive(started_s: i64, wrote: &[Wrote], lines: u32) -> Measurement<RunTiming> {
    let first = match wrote.first() {
        Some(w) => w,
        None => return Measurement::nothing_to_measure("nothing was written"),
    };

    let mut earliest = first.mtime_s;
    let mut latest = first.mtime_s;

    for w in wrote {
        if w.mtime_s < earliest {
            earliest = w.mtime_s;
        }
        if w.mtime_s > latest {
            latest = w.mtime_s;
        }
    }

    let ttfw_s = earliest.saturating_sub(started_s);
    let span_s = latest.saturating_sub(earliest);

    let lines_per_min = if span_s > 0 {
        Measurement::observed((lines as f64 * 60.0) / (span_s as f64))
    } else {
        Measurement::nothing_to_measure("instant writing is not a rate; span is zero")
    };

    Measurement::observed(RunTiming {
        ttfw_s,
        span_s,
        lines,
        lines_per_min,
    })
}

#[cfg(test)]
mod tests {
    use super::{Wrote, derive};
    use crate::measurement::{Absent, Measurement};

    #[test]
    fn clause_1_and_2_ttfw_and_span_pin_out_of_order_files() {
        // Directory listing has no order; earliest and latest must be derived
        // from the entire collection regardless of input sequence.
        let files = [
            Wrote {
                path: "src/middle.rs".to_string(),
                mtime_s: 150,
            },
            Wrote {
                path: "src/first.rs".to_string(),
                mtime_s: 120,
            },
            Wrote {
                path: "src/last.rs".to_string(),
                mtime_s: 200,
            },
        ];

        let res = derive(100, &files, 40);
        let timing = match res {
            Measurement::Observed(t) => t,
            Measurement::Missing(absent) => panic!("expected Observed, got Missing({absent:?})"),
        };

        // Earliest is 120 -> 120 - 100 = 20
        assert_eq!(timing.ttfw_s, 20);
        // Latest is 200, earliest is 120 -> 200 - 120 = 80
        assert_eq!(timing.span_s, 80);
        assert_eq!(timing.lines, 40);
        // lines * 60 / span_s = 40 * 60 / 80 = 30.0
        assert_eq!(timing.lines_per_min, Measurement::Observed(30.0));
    }

    #[test]
    fn clause_3_lines_per_min_floating_point_rate() {
        // Pin floating-point rate calculation to catch integer division bugs.
        let files = [
            Wrote {
                path: "src/a.rs".to_string(),
                mtime_s: 100,
            },
            Wrote {
                path: "src/b.rs".to_string(),
                mtime_s: 220,
            },
        ];

        let res = derive(100, &files, 1);
        let timing = match res {
            Measurement::Observed(t) => t,
            Measurement::Missing(absent) => panic!("expected Observed, got Missing({absent:?})"),
        };

        assert_eq!(timing.span_s, 120);
        // 1 * 60 / 120 = 0.5 (integer division would erroneously yield 0.0)
        assert_eq!(timing.lines_per_min, Measurement::Observed(0.5));
    }

    #[test]
    fn clause_4_single_file_span_zero_rate_missing() {
        let files = [Wrote {
            path: "src/only.rs".to_string(),
            mtime_s: 150,
        }];

        let res = derive(100, &files, 15);
        let timing = match res {
            Measurement::Observed(t) => t,
            Measurement::Missing(absent) => panic!("expected Observed, got Missing({absent:?})"),
        };

        assert_eq!(timing.ttfw_s, 50);
        assert_eq!(timing.span_s, 0);
        assert_eq!(timing.lines, 15);

        // lines_per_min is Missing when span_s == 0, with non-empty reason
        match timing.lines_per_min {
            Measurement::Observed(rate) => {
                panic!("expected Missing rate for span 0, got Observed({rate})")
            }
            Measurement::Missing(absent) => {
                let reason = match absent {
                    Absent::NothingToMeasure { reason }
                    | Absent::InstrumentFailed { reason }
                    | Absent::Untrusted { reason } => reason,
                    Absent::NotAttempted => panic!("expected reason, got NotAttempted"),
                };
                assert!(!reason.trim().is_empty(), "reason must not be empty");
                let lower = reason.to_lowercase();
                assert!(
                    lower.contains("rate")
                        || lower.contains("span")
                        || lower.contains("instant")
                        || lower.contains("zero"),
                    "reason must convey instant writing is not a rate / span is zero: {reason}"
                );
            }
        }
    }

    #[test]
    fn clause_4_two_simultaneous_files_span_zero_rate_missing() {
        // Pin separately: two files with the same mtime also give span == 0
        let files = [
            Wrote {
                path: "src/a.rs".to_string(),
                mtime_s: 200,
            },
            Wrote {
                path: "src/b.rs".to_string(),
                mtime_s: 200,
            },
        ];

        let res = derive(180, &files, 25);
        let timing = match res {
            Measurement::Observed(t) => t,
            Measurement::Missing(absent) => panic!("expected Observed, got Missing({absent:?})"),
        };

        assert_eq!(timing.ttfw_s, 20);
        assert_eq!(timing.span_s, 0);
        assert_eq!(timing.lines, 25);
        assert!(!timing.lines_per_min.is_observed());
    }

    #[test]
    fn clause_5_empty_wrote_returns_missing() {
        let res = derive(100, &[], 50);
        match res {
            Measurement::Observed(t) => {
                panic!("expected Missing for empty wrote, got Observed({t:?})")
            }
            Measurement::Missing(absent) => {
                let reason = match absent {
                    Absent::NothingToMeasure { reason }
                    | Absent::InstrumentFailed { reason }
                    | Absent::Untrusted { reason } => reason,
                    Absent::NotAttempted => panic!("expected reason, got NotAttempted"),
                };
                assert!(!reason.trim().is_empty(), "reason must not be empty");
                let lower = reason.to_lowercase();
                assert!(
                    lower.contains("nothing")
                        || lower.contains("no file")
                        || lower.contains("empty")
                        || lower.contains("wrote")
                        || lower.contains("written"),
                    "reason must convey nothing was written: {reason}"
                );
            }
        }
    }

    #[test]
    fn clauses_4_and_5_tested_together_contrast_missing_levels() {
        // Both clauses return Missing, but at different levels of the hierarchy:
        // - Clause 5: wrote is empty -> outer Measurement<RunTiming> is Missing
        // - Clause 4: span_s == 0 -> outer Measurement<RunTiming> is Observed,
        //   only the inner lines_per_min is Missing.
        // Testing them together prevents an implementation from collapsing both into
        // either always-Missing or always-Observed.
        let empty_res = derive(100, &[], 10);
        assert!(
            !empty_res.is_observed(),
            "empty wrote must make the outer derivation Missing"
        );

        let single_file = [Wrote {
            path: "src/lib.rs".to_string(),
            mtime_s: 110,
        }];
        let single_res = derive(100, &single_file, 10);
        assert!(
            single_res.is_observed(),
            "single file write must be an Observed RunTiming"
        );
        let timing = match single_res {
            Measurement::Observed(t) => t,
            Measurement::Missing(_) => unreachable!(),
        };
        assert!(
            !timing.lines_per_min.is_observed(),
            "rate must be Missing when span is 0"
        );
    }

    #[test]
    fn clause_6_file_before_start_yields_negative_ttfw_preserved_verbatim() {
        // When a file mtime is before started_s, ttfw_s must be negative and preserved verbatim.
        // Clamping to zero would mask a broken clock as an instant writer.
        let files = [Wrote {
            path: "src/clock_skew.rs".to_string(),
            mtime_s: 970,
        }];

        let res = derive(1000, &files, 10);
        let timing = match res {
            Measurement::Observed(t) => t,
            Measurement::Missing(absent) => panic!("expected Observed, got Missing({absent:?})"),
        };

        assert_eq!(timing.ttfw_s, -30);
    }

    #[test]
    fn clause_7_and_boundary_zero_lines_with_positive_span() {
        // lines == 0 with a positive span yields Observed(0.0) rate.
        // lines is passed through unchanged.
        let files = [
            Wrote {
                path: "src/a.rs".to_string(),
                mtime_s: 100,
            },
            Wrote {
                path: "src/b.rs".to_string(),
                mtime_s: 160,
            },
        ];

        let res = derive(100, &files, 0);
        let timing = match res {
            Measurement::Observed(t) => t,
            Measurement::Missing(absent) => panic!("expected Observed, got Missing({absent:?})"),
        };

        assert_eq!(timing.lines, 0);
        assert_eq!(timing.span_s, 60);
        assert_eq!(timing.lines_per_min, Measurement::Observed(0.0));
    }

    #[test]
    fn boundary_started_equal_to_earliest_mtime() {
        // started_s exactly equal to earliest mtime_s: ttfw_s == 0, observed.
        let files = [Wrote {
            path: "src/instant.rs".to_string(),
            mtime_s: 500,
        }];

        let res = derive(500, &files, 5);
        let timing = match res {
            Measurement::Observed(t) => t,
            Measurement::Missing(absent) => panic!("expected Observed, got Missing({absent:?})"),
        };

        assert_eq!(timing.ttfw_s, 0);
    }

    #[test]
    fn composition_consistent_timestamps_aggregate() {
        // ttfw_s, span_s, and lines_per_min come from the same timestamps.
        let files = [
            Wrote {
                path: "src/b.rs".to_string(),
                mtime_s: 140,
            },
            Wrote {
                path: "src/a.rs".to_string(),
                mtime_s: 110,
            },
            Wrote {
                path: "src/c.rs".to_string(),
                mtime_s: 170,
            },
        ];

        let started_s = 100;
        let res = derive(started_s, &files, 30);
        let timing = match res {
            Measurement::Observed(t) => t,
            Measurement::Missing(absent) => panic!("expected Observed, got Missing({absent:?})"),
        };

        // started_s + ttfw_s + span_s == latest mtime
        assert_eq!(started_s + timing.ttfw_s + timing.span_s, 170);
    }
}
