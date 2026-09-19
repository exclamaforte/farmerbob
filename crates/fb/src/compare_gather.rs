//! Comparison row gathering from score files.
//!
//! This module reads candidate figures from `<task>.score.json` files and converts
//! them into [`Row`] structures for comparison rendering by [`crate::compare_cmd`].
//! It ensures that unmeasured figures are preserved as [`Measurement::Missing`] rather
//! than collapsed into zero or placeholder values.
//!
//! The module is not yet wired into the CLI; wiring is a separate task.
//! `#[allow(dead_code)]` on public items avoids false positives from `-D warnings`.

#![allow(dead_code)]

use std::io::Write;
use std::path::Path;

use farmerbob_core::measurement::Measurement;

use crate::compare_cmd::{self, Row};

/// Why a score file could not be turned into rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatherError {
    /// The file does not exist or could not be read. Carries a non-empty reason.
    Unreadable(String),
    /// The file is not a JSON array of objects. Carries a non-empty reason.
    Malformed(String),
}

/// Read a task's score file into comparison rows.
///
/// Every figure the scorer did not measure becomes `Missing`, never the
/// number sitting beside the flag.
pub fn rows_for(score_path: &Path) -> Result<Vec<Row>, GatherError> {
    let content = match std::fs::read_to_string(score_path) {
        Ok(c) => c,
        Err(e) => {
            return Err(GatherError::Unreadable(format!(
                "could not read score file at {}: {e}",
                score_path.display()
            )));
        }
    };

    if content.trim().is_empty() {
        return Err(GatherError::Malformed(
            "file contains only whitespace".to_string(),
        ));
    }

    let value: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            return Err(GatherError::Malformed(format!(
                "invalid JSON in {}: {e}",
                score_path.display()
            )));
        }
    };

    let Some(array) = value.as_array() else {
        return Err(GatherError::Malformed(
            "top level JSON value is not an array".to_string(),
        ));
    };

    let mut rows = Vec::with_capacity(array.len());

    for (i, element) in array.iter().enumerate() {
        let Some(obj) = element.as_object() else {
            return Err(GatherError::Malformed(format!(
                "element at index {i} is not a JSON object"
            )));
        };

        let arm = obj
            .get("source")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();

        let tests = match obj.get("tests_run") {
            Some(serde_json::Value::Number(n)) => {
                match n.as_u64().and_then(|v| u32::try_from(v).ok()) {
                    Some(count) => Measurement::observed(count),
                    None => Measurement::not_attempted(),
                }
            }
            _ => Measurement::not_attempted(),
        };

        let lines_measured = obj
            .get("lines_measured")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let lines = if lines_measured {
            match obj.get("lines") {
                Some(serde_json::Value::Number(n)) => {
                    if let Some(v) = n.as_u64() {
                        Measurement::observed(u32::try_from(v).unwrap_or(u32::MAX))
                    } else {
                        Measurement::not_attempted()
                    }
                }
                _ => Measurement::not_attempted(),
            }
        } else {
            Measurement::not_attempted()
        };

        let clippy_measured = obj
            .get("clippy_measured")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let clippy = if clippy_measured {
            match obj.get("clippy") {
                Some(serde_json::Value::Number(n)) => {
                    if let Some(v) = n.as_u64() {
                        Measurement::observed(u32::try_from(v).unwrap_or(u32::MAX))
                    } else {
                        Measurement::not_attempted()
                    }
                }
                Some(serde_json::Value::String(s)) => {
                    if let Ok(v) = s.parse::<u32>() {
                        Measurement::observed(v)
                    } else {
                        Measurement::not_attempted()
                    }
                }
                _ => Measurement::not_attempted(),
            }
        } else {
            Measurement::not_attempted()
        };

        rows.push(Row {
            arm,
            tests,
            lines,
            clippy,
        });
    }

    Ok(rows)
}

/// Read and render. Returns the exit code the caller should use.
pub fn run(score_path: &Path, out: &mut dyn Write) -> i32 {
    match rows_for(score_path) {
        Ok(rows) => compare_cmd::run(&rows, out),
        Err(GatherError::Unreadable(reason)) => {
            let _ = writeln!(out, "{}: unreadable: {reason}", score_path.display());
            2
        }
        Err(GatherError::Malformed(reason)) => {
            let _ = writeln!(out, "{}: malformed: {reason}", score_path.display());
            3
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempFixture {
        path: std::path::PathBuf,
    }

    impl TempFixture {
        fn new(contents: &str) -> Self {
            let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "fb_compare_gather_test_{}_{}_{}.score.json",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos(),
                n
            ));
            std::fs::write(&path, contents).expect("write temp fixture");
            Self { path }
        }
    }

    impl Drop for TempFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    #[test]
    fn clause_1_all_measured_figures_observed_and_arm_carried() {
        let json = r#"[
            {
                "source": "candidate-arm",
                "tests_run": 42,
                "lines": 120,
                "lines_measured": true,
                "clippy": 5,
                "clippy_measured": true
            }
        ]"#;
        let fixture = TempFixture::new(json);
        let rows = rows_for(&fixture.path).expect("parse rows");
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.arm, "candidate-arm");
        assert_eq!(row.tests.value(), Some(&42));
        assert_eq!(row.lines.value(), Some(&120));
        assert_eq!(row.clippy.value(), Some(&5));
    }

    #[test]
    fn clause_2_lines_measured_false_makes_lines_missing_despite_number() {
        let json = r#"[
            {
                "source": "armA",
                "tests_run": 10,
                "lines": 99,
                "lines_measured": false,
                "clippy": 0,
                "clippy_measured": true
            }
        ]"#;
        let fixture = TempFixture::new(json);
        let rows = rows_for(&fixture.path).expect("parse rows");
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].lines.is_observed());
        assert_eq!(rows[0].lines.value(), None);
        assert_eq!(rows[0].tests.value(), Some(&10));
        assert_eq!(rows[0].clippy.value(), Some(&0));
    }

    #[test]
    fn clause_3_clippy_measured_false_makes_clippy_missing_despite_number() {
        let json = r#"[
            {
                "source": "armB",
                "tests_run": 10,
                "lines": 50,
                "lines_measured": true,
                "clippy": 15,
                "clippy_measured": false
            }
        ]"#;
        let fixture = TempFixture::new(json);
        let rows = rows_for(&fixture.path).expect("parse rows");
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].clippy.is_observed());
        assert_eq!(rows[0].clippy.value(), None);
        assert_eq!(rows[0].tests.value(), Some(&10));
        assert_eq!(rows[0].lines.value(), Some(&50));
    }

    #[test]
    fn clause_4_absent_lines_measured_treated_as_false() {
        let json = r#"[
            {
                "source": "armC",
                "tests_run": 10,
                "lines": 75,
                "clippy": 0,
                "clippy_measured": true
            }
        ]"#;
        let fixture = TempFixture::new(json);
        let rows = rows_for(&fixture.path).expect("parse rows");
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].lines.is_observed());
        assert_eq!(rows[0].lines.value(), None);
    }

    #[test]
    fn clause_5_and_6_tests_run_missing_vs_zero_observed() {
        let json = r#"[
            {
                "source": "arm_missing_tests",
                "lines": 10,
                "lines_measured": true
            },
            {
                "source": "arm_non_numeric_tests",
                "tests_run": "not-a-number",
                "lines": 10,
                "lines_measured": true
            },
            {
                "source": "arm_zero_tests",
                "tests_run": 0,
                "lines": 10,
                "lines_measured": true
            }
        ]"#;
        let fixture = TempFixture::new(json);
        let rows = rows_for(&fixture.path).expect("parse rows");
        assert_eq!(rows.len(), 3);

        // Clause 5: missing tests_run is Missing, not zero
        assert_eq!(rows[0].arm, "arm_missing_tests");
        assert!(!rows[0].tests.is_observed());
        assert_eq!(rows[0].tests.value(), None);

        // Clause 5: non-numeric tests_run is Missing, not zero
        assert_eq!(rows[1].arm, "arm_non_numeric_tests");
        assert!(!rows[1].tests.is_observed());
        assert_eq!(rows[1].tests.value(), None);

        // Clause 6: tests_run: 0 with field present is Observed(0)
        assert_eq!(rows[2].arm, "arm_zero_tests");
        assert!(rows[2].tests.is_observed());
        assert_eq!(rows[2].tests.value(), Some(&0));
    }

    #[test]
    fn clause_7_rows_returned_in_order_with_none_dropped() {
        let json = r#"[
            {
                "source": "arm1",
                "tests_run": 5
            },
            {
                "source": "arm2_all_missing"
            },
            {
                "source": "arm3",
                "tests_run": 10
            }
        ]"#;
        let fixture = TempFixture::new(json);
        let rows = rows_for(&fixture.path).expect("parse rows");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].arm, "arm1");
        assert_eq!(rows[1].arm, "arm2_all_missing");
        assert!(!rows[1].tests.is_observed());
        assert!(!rows[1].lines.is_observed());
        assert!(!rows[1].clippy.is_observed());
        assert_eq!(rows[2].arm, "arm3");
    }

    #[test]
    fn clause_8_unreadable_and_malformed_differ() {
        let nonexistent = std::env::temp_dir().join(format!(
            "fb_nonexistent_{}_{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let unreadable_err = rows_for(&nonexistent).expect_err("nonexistent file must error");
        assert!(matches!(unreadable_err, GatherError::Unreadable(_)));

        let not_an_array = TempFixture::new(r#"{"source": "arm1"}"#);
        let malformed_err = rows_for(&not_an_array.path).expect_err("non-array must error");
        assert!(matches!(malformed_err, GatherError::Malformed(_)));

        assert_ne!(unreadable_err, malformed_err);
    }

    #[test]
    fn clause_9_run_exit_codes_for_success_and_errors() {
        let json = r#"[
            {
                "source": "arm1",
                "tests_run": 1,
                "lines": 10,
                "lines_measured": true,
                "clippy": 0,
                "clippy_measured": true
            }
        ]"#;
        let fixture = TempFixture::new(json);
        let mut out = Vec::new();
        let code_success = run(&fixture.path, &mut out);
        assert_eq!(code_success, 0);

        let empty_fixture = TempFixture::new("[]");
        let mut empty_out = Vec::new();
        let code_empty = run(&empty_fixture.path, &mut empty_out);
        assert_eq!(code_empty, 1);

        let nonexistent = std::env::temp_dir().join(format!(
            "fb_nonexistent_run_{}_{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let mut unreadable_out = Vec::new();
        let code_unreadable = run(&nonexistent, &mut unreadable_out);

        let malformed_fixture = TempFixture::new("{not json}");
        let mut malformed_out = Vec::new();
        let code_malformed = run(&malformed_fixture.path, &mut malformed_out);

        assert_ne!(code_unreadable, 0);
        assert_ne!(code_unreadable, 1);
        assert_ne!(code_malformed, 0);
        assert_ne!(code_malformed, 1);
        assert_ne!(code_unreadable, code_malformed);
    }

    #[test]
    fn clause_10_run_output_success_vs_failure() {
        let json = r#"[
            {
                "source": "arm_rendered",
                "tests_run": 5,
                "lines": 20,
                "lines_measured": true,
                "clippy": 0,
                "clippy_measured": true
            }
        ]"#;
        let fixture = TempFixture::new(json);
        let mut success_out = Vec::new();
        let code = run(&fixture.path, &mut success_out);
        assert_eq!(code, 0);
        let success_text = String::from_utf8_lossy(&success_out);
        assert!(success_text.contains("arm_rendered"));
        assert!(success_text.contains("arm"));
        assert!(success_text.contains("tests"));

        let nonexistent = std::env::temp_dir().join(format!(
            "fb_nonexistent_msg_{}_{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let mut failure_out = Vec::new();
        let fail_code = run(&nonexistent, &mut failure_out);
        assert_ne!(fail_code, 0);
        let failure_text = String::from_utf8_lossy(&failure_out);
        assert!(failure_text.contains(&nonexistent.to_string_lossy().to_string()));
    }

    #[test]
    fn boundaries_empty_array_whitespace_and_non_object_element() {
        let empty_fixture = TempFixture::new("[]");
        let rows = rows_for(&empty_fixture.path).expect("empty array is Ok");
        assert!(rows.is_empty());

        let whitespace_fixture = TempFixture::new("   \n\t  ");
        let ws_err = rows_for(&whitespace_fixture.path).expect_err("whitespace is Malformed");
        assert!(matches!(ws_err, GatherError::Malformed(_)));

        let non_obj_fixture = TempFixture::new(r#"[42]"#);
        let non_obj_err =
            rows_for(&non_obj_fixture.path).expect_err("non-object element is Malformed");
        assert!(matches!(non_obj_err, GatherError::Malformed(_)));
    }

    #[test]
    fn boundary_lines_value_exceeding_u32_width() {
        let json = r#"[
            {
                "source": "big_lines_arm",
                "lines": 99999999999999999,
                "lines_measured": true
            }
        ]"#;
        let fixture = TempFixture::new(json);
        let rows = rows_for(&fixture.path).expect("parse rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].lines.value(), Some(&u32::MAX));
    }

    #[test]
    fn boundary_missing_source_defaults_to_empty() {
        let json = r#"[
            {
                "tests_run": 7,
                "lines": 10,
                "lines_measured": true
            }
        ]"#;
        let fixture = TempFixture::new(json);
        let rows = rows_for(&fixture.path).expect("parse rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].arm, "");
        assert_eq!(rows[0].tests.value(), Some(&7));
    }

    #[test]
    fn composition_lines_observed_and_clippy_missing() {
        let json = r#"[
            {
                "source": "mixed_arm",
                "lines": 88,
                "lines_measured": true,
                "clippy_measured": false
            }
        ]"#;
        let fixture = TempFixture::new(json);
        let rows = rows_for(&fixture.path).expect("parse rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].lines.value(), Some(&88));
        assert!(!rows[0].clippy.is_observed());
    }

    #[test]
    fn scorer_string_clippy_representation() {
        let json = r#"[
            {
                "source": "string_clippy_arm",
                "tests_run": 10,
                "lines": 50,
                "lines_measured": true,
                "clippy": "0",
                "clippy_measured": true
            }
        ]"#;
        let fixture = TempFixture::new(json);
        let rows = rows_for(&fixture.path).expect("parse rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].clippy.value(), Some(&0));
    }
}
