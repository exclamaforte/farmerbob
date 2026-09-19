//! Comparison table rendering for candidate runs.
//!
//! This module provides the [`Row`] type and functions to render a side-by-side
//! comparison table of candidate measurements. It uses
//! [`farmerbob_core::measurement::Measurement`] to distinguish observed values
//! from absent ones, ensuring a measured zero is never collapsed with an
//! unmeasured field.
//!
//! The module is not yet wired into the CLI; wiring is a separate task.
//! `#[allow(dead_code)]` on public items avoids false positives from `-D warnings`.

use std::io::Write;

use farmerbob_core::measurement::Measurement;

/// One candidate's measured figures, as the caller read them.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Arm name.
    pub arm: String,
    /// Tests that passed, or `Missing` when the count could not be read.
    pub tests: Measurement<u32>,
    /// Lines added to the declared deliverable, or `Missing`.
    pub lines: Measurement<u32>,
    /// Clippy warnings above the crate's baseline, or `Missing`.
    pub clippy: Measurement<u32>,
}

/// The marker a `Missing` figure renders as. Exactly this string.
#[allow(dead_code)]
pub const ABSENT: &str = "--";

/// Render a comparison table.
///
/// Columns are arm, tests, lines, clippy, in that order. A `Missing` figure
/// renders as a marker that is NOT a number and NOT blank, so an absent
/// reading cannot be mistaken for a zero.
#[allow(dead_code)]
pub fn table(rows: &[Row]) -> String {
    let mut out = String::new();
    // Heading line mentioning each column name case-insensitively
    out.push_str("arm  tests  lines  clippy\n");
    for row in rows {
        out.push_str(&format_row(row));
        out.push('\n');
    }
    // Remove trailing newline if there were rows, but keep heading for empty
    if !rows.is_empty() {
        out.pop();
    }
    out
}

#[allow(dead_code)]
fn format_row(row: &Row) -> String {
    let tests_str = format_measurement(&row.tests);
    let lines_str = format_measurement(&row.lines);
    let clippy_str = format_measurement(&row.clippy);
    format!("{}  {}  {}  {}", row.arm, tests_str, lines_str, clippy_str)
}

#[allow(dead_code)]
fn format_measurement(m: &Measurement<u32>) -> String {
    match m {
        Measurement::Observed(v) => v.to_string(),
        Measurement::Missing(_) => ABSENT.to_string(),
    }
}

/// Render and write. Returns the exit code the caller should use.
///
/// - `0` — at least one row was rendered.
/// - `1` — no rows. The table is a heading and nothing else, and that is still printed.
#[allow(dead_code)]
pub fn run(rows: &[Row], out: &mut dyn Write) -> i32 {
    let rendered = table(rows);
    let _ = out.write_all(rendered.as_bytes());
    if rows.is_empty() {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn observed_row(arm: &str, tests: u32, lines: u32, clippy: u32) -> Row {
        Row {
            arm: arm.to_string(),
            tests: Measurement::observed(tests),
            lines: Measurement::observed(lines),
            clippy: Measurement::observed(clippy),
        }
    }

    fn missing_row(arm: &str) -> Row {
        Row {
            arm: arm.to_string(),
            tests: Measurement::not_attempted(),
            lines: Measurement::not_attempted(),
            clippy: Measurement::not_attempted(),
        }
    }

    fn row_with_mixed(arm: &str, tests: Measurement<u32>, lines: Measurement<u32>, clippy: Measurement<u32>) -> Row {
        Row { arm: arm.to_string(), tests, lines, clippy }
    }

    #[test]
    fn clause_1_all_observed_renders_numbers() {
        let rows = [observed_row("armA", 10, 50, 3)];
        let out = table(&rows);
        // Heading + one data line
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].to_lowercase().contains("arm"));
        assert!(lines[0].to_lowercase().contains("tests"));
        assert!(lines[0].to_lowercase().contains("lines"));
        assert!(lines[0].to_lowercase().contains("clippy"));
        assert!(lines[1].contains("armA"));
        assert!(lines[1].contains("10"));
        assert!(lines[1].contains("50"));
        assert!(lines[1].contains("3"));
    }

    #[test]
    fn clause_2_missing_renders_as_absent_marker() {
        let rows = [missing_row("armB")];
        let out = table(&rows);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        // All three figures should be ABSENT (--)
        assert!(lines[1].contains(ABSENT));
        // Count occurrences of ABSENT in the data line
        let absent_count = lines[1].matches(ABSENT).count();
        assert_eq!(absent_count, 3);
    }

    #[test]
    fn clause_3_observed_zero_renders_as_zero_not_absent() {
        let rows = [observed_row("armC", 0, 0, 0)];
        let out = table(&rows);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        // Zero should render as "0", not ABSENT
        assert!(lines[1].contains("0"));
        assert!(!lines[1].contains(ABSENT));
        // And we should see three zeros (tests, lines, clippy)
        let zero_count = lines[1].matches("0").count();
        assert_eq!(zero_count, 3);
    }

    #[test]
    fn clause_3_pair_zero_vs_missing_are_different() {
        let zero_row = observed_row("armZ", 0, 0, 0);
        let missing_row = missing_row("armM");
        let zero_out = table(&[zero_row]);
        let missing_out = table(&[missing_row]);
        let zero_lines: Vec<&str> = zero_out.lines().collect();
        let missing_lines: Vec<&str> = missing_out.lines().collect();
        // Zero row has numbers, missing row has ABSENT markers
        assert!(!zero_lines[1].contains(ABSENT));
        assert!(missing_lines[1].contains(ABSENT));
        assert!(zero_lines[1].contains("0"));
        assert!(!missing_lines[1].contains("0"));
    }

    #[test]
    fn clause_4_rows_appear_in_input_order_once() {
        let rows = [
            observed_row("first", 1, 1, 1),
            observed_row("second", 2, 2, 2),
            observed_row("third", 3, 3, 3),
        ];
        let out = table(&rows);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 4); // heading + 3 rows
        assert!(lines[1].contains("first"));
        assert!(lines[2].contains("second"));
        assert!(lines[3].contains("third"));
    }

    #[test]
    fn clause_5_heading_mentions_all_four_columns() {
        let rows = [observed_row("arm", 1, 2, 3)];
        let out = table(&rows);
        let heading = out.lines().next().unwrap();
        let lower = heading.to_lowercase();
        assert!(lower.contains("arm"));
        assert!(lower.contains("tests"));
        assert!(lower.contains("lines"));
        assert!(lower.contains("clippy"));
    }

    #[test]
    fn clause_6_run_writes_table_and_returns_0_for_nonempty() {
        let rows = [observed_row("arm", 1, 2, 3)];
        let mut buf = Cursor::new(Vec::new());
        let code = run(&rows, &mut buf);
        assert_eq!(code, 0);
        let written = String::from_utf8(buf.into_inner()).unwrap();
        assert_eq!(written, table(&rows));
    }

    #[test]
    fn clause_7_empty_input_returns_1_and_writes_heading() {
        let rows: Vec<Row> = vec![];
        let mut buf = Cursor::new(Vec::new());
        let code = run(&rows, &mut buf);
        assert_eq!(code, 1);
        let written = String::from_utf8(buf.into_inner()).unwrap();
        // Should still have the heading
        assert!(written.contains("arm"));
        assert!(written.contains("tests"));
        assert!(written.contains("lines"));
        assert!(written.contains("clippy"));
        // But no data lines
        let lines: Vec<&str> = written.lines().collect();
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn clause_8_duplicate_arm_names_both_appear() {
        let rows = [
            observed_row("same", 1, 1, 1),
            observed_row("same", 2, 2, 2),
        ];
        let out = table(&rows);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3); // heading + 2 rows
        // Both rows appear
        assert!(lines[1].contains("1"));
        assert!(lines[2].contains("2"));
    }

    #[test]
    fn boundary_all_three_missing_renders_three_absent() {
        let rows = [missing_row("armAllMissing")];
        let out = table(&rows);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        let absent_count = lines[1].matches(ABSENT).count();
        assert_eq!(absent_count, 3);
    }

    #[test]
    fn boundary_u32_max_renders_correctly() {
        let rows = [Row {
            arm: "max".to_string(),
            tests: Measurement::observed(u32::MAX),
            lines: Measurement::observed(u32::MAX),
            clippy: Measurement::observed(u32::MAX),
        }];
        let out = table(&rows);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        let max_str = u32::MAX.to_string();
        assert!(lines[1].contains(&max_str));
        // Should appear three times
        let count = lines[1].matches(&max_str).count();
        assert_eq!(count, 3);
    }

    #[test]
    fn boundary_empty_arm_name_renders() {
        let rows = [observed_row("", 5, 10, 2)];
        let out = table(&rows);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        // Empty arm name should still produce a line (starts with spaces for the arm column)
        assert!(lines[1].contains("5"));
        assert!(lines[1].contains("10"));
        assert!(lines[1].contains("2"));
    }

    #[test]
    fn absent_constant_is_exactly_double_dash() {
        assert_eq!(ABSENT, "--");
    }

    #[test]
    fn measurement_variants_all_render_correctly() {
        let rows = [
            row_with_mixed("not_attempted", Measurement::not_attempted(), Measurement::observed(1), Measurement::observed(2)),
            row_with_mixed("instrument_failed", Measurement::instrument_failed("boom"), Measurement::observed(3), Measurement::observed(4)),
            row_with_mixed("nothing_to_measure", Measurement::nothing_to_measure("none"), Measurement::observed(5), Measurement::observed(6)),
            row_with_mixed("untrusted", Measurement::untrusted("bad"), Measurement::observed(7), Measurement::observed(8)),
        ];
        let out = table(&rows);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 5); // heading + 4 rows
        // First column (tests) should be ABSENT for all four
        for (i, line) in lines.iter().enumerate().take(5).skip(1) {
            assert!(line.contains(ABSENT), "row {i} should contain ABSENT in tests column");
        }
    }
}