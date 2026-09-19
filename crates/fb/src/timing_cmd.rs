//! Timing table rendering from run file timestamps.
//!
//! Ports `fb-timing.sh` to use `farmerbob_core::run_timing::derive` for all
//! time and throughput arithmetic, ensuring the shell and core agree.
#![allow(dead_code)]
// Uncalled pub fn in a binary crate trips dead_code under -D warnings;
// wiring into main.rs's argument parser is a separate task.

use std::io::Write;

use farmerbob_core::measurement::Measurement;
use farmerbob_core::run_timing::{self, Wrote};

/// One run to report on.
#[derive(Debug, Clone, PartialEq)]
pub struct TimingRow {
    /// Arm name.
    pub arm: String,
    /// When the run began, seconds since the epoch.
    pub started_s: i64,
    /// The files it wrote, with their mtimes, in any order.
    pub wrote: Vec<Wrote>,
    /// Lines added across those files, as the caller counted them.
    pub lines: u32,
}

/// The marker an absent figure renders as. Exactly this string.
pub const ABSENT: &str = "--";

/// Render a timing table.
///
/// Columns are arm, ttfw, span, lines, rate, in that order. A `Missing`
/// figure renders as `ABSENT`, never as a number and never blank.
pub fn table(rows: &[TimingRow]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{:<22} {:>7} {:>7} {:>7} {:>9}\n",
        "ARM", "TTFW", "SPAN", "LINES", "RATE"
    ));

    for row in rows {
        let measurement = run_timing::derive(row.started_s, &row.wrote, row.lines);
        let (ttfw, span, lines, rate) = match measurement {
            Measurement::Observed(timing) => {
                let rate_str = match timing.lines_per_min {
                    Measurement::Observed(r) => format!("{r}"),
                    Measurement::Missing(_) => ABSENT.to_string(),
                };
                (
                    timing.ttfw_s.to_string(),
                    timing.span_s.to_string(),
                    timing.lines.to_string(),
                    rate_str,
                )
            }
            Measurement::Missing(_) => (
                ABSENT.to_string(),
                ABSENT.to_string(),
                ABSENT.to_string(),
                ABSENT.to_string(),
            ),
        };
        out.push_str(&format!(
            "{:<22} {:>7} {:>7} {:>7} {:>9}\n",
            row.arm, ttfw, span, lines, rate
        ));
    }

    out
}

/// Render and write. Returns the exit code the caller should use.
pub fn run(rows: &[TimingRow], out: &mut dyn Write) -> i32 {
    let rendered = table(rows);
    let _ = out.write_all(rendered.as_bytes());
    if rows.is_empty() { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use farmerbob_core::run_timing::{Wrote, derive};

    #[test]
    fn clause_1_non_empty_row_matches_derive_output() {
        let files = [
            Wrote {
                path: "src/a.rs".to_string(),
                mtime_s: 120,
            },
            Wrote {
                path: "src/b.rs".to_string(),
                mtime_s: 200,
            },
        ];
        let row = TimingRow {
            arm: "test-arm-1".to_string(),
            started_s: 100,
            wrote: files.to_vec(),
            lines: 40,
        };

        let expected_meas = derive(row.started_s, &row.wrote, row.lines);
        let expected = match expected_meas {
            Measurement::Observed(t) => t,
            Measurement::Missing(_) => unreachable!(),
        };

        let output = table(&[row]);
        assert!(output.contains("test-arm-1"));
        assert!(output.contains(&expected.ttfw_s.to_string()));
        assert!(output.contains(&expected.span_s.to_string()));
        assert!(output.contains(&expected.lines.to_string()));
        if let Measurement::Observed(rate) = expected.lines_per_min {
            assert!(output.contains(&rate.to_string()));
        }
    }

    #[test]
    fn clause_2_and_3_pinned_as_pair_contrast_absent_levels() {
        // Clause 2: empty wrote renders with ABSENT figures
        let empty_row = TimingRow {
            arm: "empty-arm".to_string(),
            started_s: 100,
            wrote: Vec::new(),
            lines: 0,
        };
        let out_empty = table(&[empty_row]);
        assert!(out_empty.contains("empty-arm"));
        assert!(out_empty.contains(ABSENT));

        // Clause 3: single file has observed ttfw and span (0), but ABSENT rate
        let single_file = [Wrote {
            path: "src/one.rs".to_string(),
            mtime_s: 150,
        }];
        let single_row = TimingRow {
            arm: "single-arm".to_string(),
            started_s: 100,
            wrote: single_file.to_vec(),
            lines: 15,
        };
        let out_single = table(&[single_row]);
        assert!(out_single.contains("single-arm"));
        assert!(out_single.contains("50")); // ttfw: 150 - 100
        assert!(out_single.contains(ABSENT)); // rate is absent

        // Clause 4: verify that they render differently as a pair
        assert_ne!(out_empty, out_single);
    }

    #[test]
    fn clause_5_input_order_preserved_exactly() {
        let rows = [
            TimingRow {
                arm: "arm-first".to_string(),
                started_s: 100,
                wrote: Vec::new(),
                lines: 0,
            },
            TimingRow {
                arm: "arm-second".to_string(),
                started_s: 100,
                wrote: Vec::new(),
                lines: 0,
            },
            TimingRow {
                arm: "arm-third".to_string(),
                started_s: 100,
                wrote: Vec::new(),
                lines: 0,
            },
        ];

        let out = table(&rows);
        let pos1 = out.find("arm-first").unwrap();
        let pos2 = out.find("arm-second").unwrap();
        let pos3 = out.find("arm-third").unwrap();

        assert!(pos1 < pos2);
        assert!(pos2 < pos3);
    }

    #[test]
    fn clause_6_heading_mentions_all_five_columns_case_insensitively() {
        let out = table(&[]);
        let first_line = out.lines().next().unwrap_or("").to_lowercase();

        assert!(first_line.contains("arm"));
        assert!(first_line.contains("ttfw"));
        assert!(first_line.contains("span"));
        assert!(first_line.contains("lines"));
        assert!(first_line.contains("rate"));
    }

    #[test]
    fn clause_7_run_writes_table_output_and_returns_correct_exit_codes() {
        let rows = [TimingRow {
            arm: "arm-test".to_string(),
            started_s: 100,
            wrote: Vec::new(),
            lines: 0,
        }];

        // Non-empty rows: returns 0 and writes table output
        let mut out_non_empty = Vec::new();
        let code_non_empty = run(&rows, &mut out_non_empty);
        assert_eq!(code_non_empty, 0);
        assert_eq!(String::from_utf8(out_non_empty).unwrap(), table(&rows));

        // Empty rows: returns 1 and writes heading only
        let mut out_empty = Vec::new();
        let code_empty = run(&[], &mut out_empty);
        assert_eq!(code_empty, 1);
        assert_eq!(String::from_utf8(out_empty).unwrap(), table(&[]));
    }

    #[test]
    fn clause_8_absent_constant_is_verbatim() {
        assert_eq!(ABSENT, "--");
    }

    #[test]
    fn boundary_zero_rows_heading_only() {
        let out = table(&[]);
        assert_eq!(out.lines().count(), 1);
    }

    #[test]
    fn boundary_lines_zero_with_files_written() {
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
        let row = TimingRow {
            arm: "zero-lines-arm".to_string(),
            started_s: 100,
            wrote: files.to_vec(),
            lines: 0,
        };

        let out = table(&[row]);
        assert!(out.contains("zero-lines-arm"));
        assert!(out.contains("0"));
    }

    #[test]
    fn boundary_mtime_before_started_renders_derive_output() {
        let files = [Wrote {
            path: "src/clock_skew.rs".to_string(),
            mtime_s: 970,
        }];
        let row = TimingRow {
            arm: "skew-arm".to_string(),
            started_s: 1000,
            wrote: files.to_vec(),
            lines: 10,
        };

        let expected_meas = derive(row.started_s, &row.wrote, row.lines);
        let expected = match expected_meas {
            Measurement::Observed(t) => t,
            Measurement::Missing(_) => unreachable!(),
        };

        let out = table(&[row]);
        assert!(out.contains("skew-arm"));
        assert!(out.contains(&expected.ttfw_s.to_string()));
    }

    #[test]
    fn boundary_duplicate_arm_names_both_rendered() {
        let rows = [
            TimingRow {
                arm: "same-arm".to_string(),
                started_s: 100,
                wrote: Vec::new(),
                lines: 0,
            },
            TimingRow {
                arm: "same-arm".to_string(),
                started_s: 200,
                wrote: Vec::new(),
                lines: 0,
            },
        ];

        let out = table(&rows);
        let matches = out.matches("same-arm").count();
        assert_eq!(matches, 2);
    }
}
