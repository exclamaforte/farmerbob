//! Gather verification observations from a task score file.
//!
//! This module is deliberately separate from the command parser. It reads the
//! scorer's JSON records, constructs [`crate::verify_cmd::Raw`] values, and
//! leaves both fate selection and rendering to `verify_cmd` and the core
//! verifier.
#![allow(dead_code)]

use std::io::Write;
use std::path::Path;

use crate::verify_cmd::{self, Raw};

/// Why a score file could not be turned into rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatherError {
    /// The file does not exist or could not be read. Carries a non-empty reason.
    Unreadable(String),
    /// The file is not a JSON array of objects. Carries a non-empty reason.
    Malformed(String),
}

/// Read a task's score file into the rows `verify_cmd` renders.
pub fn rows_for(score_path: &Path) -> Result<Vec<Raw>, GatherError> {
    let content = std::fs::read_to_string(score_path).map_err(|error| {
        GatherError::Unreadable(format!(
            "could not read score file at {}: {error}",
            score_path.display()
        ))
    })?;

    if content.trim().is_empty() {
        return Err(GatherError::Malformed(
            "score file contains only whitespace".to_string(),
        ));
    }

    let value: serde_json::Value = serde_json::from_str(&content)
        .map_err(|error| GatherError::Malformed(format!("invalid score JSON: {error}")))?;

    let Some(elements) = value.as_array() else {
        return Err(GatherError::Malformed(
            "top-level JSON value is not an array".to_string(),
        ));
    };

    let mut rows = Vec::with_capacity(elements.len());
    for (index, element) in elements.iter().enumerate() {
        let Some(object) = element.as_object() else {
            return Err(GatherError::Malformed(format!(
                "array element at index {index} is not an object"
            )));
        };

        // `test` is part of the score-file schema, but Raw intentionally has
        // no test-status field. The test value therefore has no independent
        // observation to preserve; the scorer's `err` is the test log fed to
        // verify_cmd's core decision.
        let _test = object.get("test").and_then(serde_json::Value::as_str);

        let arm = object
            .get("source")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let verdict = object.get("verdict").and_then(serde_json::Value::as_str);
        let build = match object.get("build").and_then(serde_json::Value::as_str) {
            Some("pass") => Some(true),
            Some("FAIL") => Some(false),
            _ => None,
        };
        let test_log = object
            .get("err")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();

        rows.push(Raw {
            arm,
            deliverable: verdict != Some("NO-OP"),
            built: build,
            test_log,
            timed_out: verdict == Some("INDETERMINATE"),
        });
    }

    Ok(rows)
}

/// Read and render. Returns the exit code the caller should use.
pub fn run(score_path: &Path, out: &mut dyn Write) -> i32 {
    match rows_for(score_path) {
        Ok(rows) => {
            // verify_cmd currently exposes its aggregate renderer as
            // `assess_field`; this is the closest honest composition to the
            // specification's reference to a future `verify_cmd::run`.
            let (lines, code) = verify_cmd::assess_field(&rows);
            for line in lines {
                let _ = writeln!(out, "{}", line.text);
            }
            code
        }
        Err(GatherError::Unreadable(reason)) => {
            let _ = writeln!(out, "{}: unreadable: {reason}", score_path.display());
            4
        }
        Err(GatherError::Malformed(reason)) => {
            let _ = writeln!(out, "{}: malformed: {reason}", score_path.display());
            5
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        path: PathBuf,
    }

    impl Fixture {
        fn new(contents: &str) -> Self {
            let serial = FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "fb_verify_gather_{}_{}_{}.score.json",
                std::process::id(),
                serial,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            std::fs::write(&path, contents).expect("write score fixture");
            Self { path }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn missing_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "fb_verify_gather_missing_{}_{}.score.json",
            std::process::id(),
            FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn rows_map_pinned_fields_and_preserve_order() {
        let fixture = Fixture::new(
            r#"[
                {"source":"alpha","build":"pass","test":"pass","verdict":"PASS","err":"kept"},
                {"source":"beta","build":"unknown","test":"FAIL","verdict":"INDETERMINATE"},
                {"source":"gamma","verdict":"NO-OP","err":""}
            ]"#,
        );

        let rows = rows_for(&fixture.path).expect("read score fixture");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].arm, "alpha");
        assert!(rows[0].deliverable);
        assert_eq!(rows[0].built, Some(true));
        assert_eq!(rows[0].test_log, "kept");
        assert!(!rows[0].timed_out);
        assert_eq!(rows[1].arm, "beta");
        assert_eq!(rows[1].built, None);
        assert!(rows[1].timed_out);
        assert_eq!(rows[2].arm, "gamma");
        assert!(!rows[2].deliverable);
        assert_eq!(rows[2].test_log, "");
    }

    #[test]
    fn empty_array_is_readable_and_non_objects_are_malformed() {
        let empty = Fixture::new("[]");
        assert_eq!(rows_for(&empty.path), Ok(Vec::new()));

        let non_object = Fixture::new("[null]");
        assert!(matches!(
            rows_for(&non_object.path),
            Err(GatherError::Malformed(reason)) if !reason.is_empty()
        ));
    }

    #[test]
    fn unreadable_and_malformed_runs_have_distinct_codes() {
        let missing = missing_path();
        let mut missing_out = Vec::new();
        let unreadable_code = run(&missing, &mut missing_out);

        let malformed = Fixture::new(" ");
        let mut malformed_out = Vec::new();
        let malformed_code = run(&malformed.path, &mut malformed_out);

        assert_ne!(unreadable_code, malformed_code);
        assert_ne!(unreadable_code, 0);
        assert_ne!(malformed_code, 0);
    }
}
