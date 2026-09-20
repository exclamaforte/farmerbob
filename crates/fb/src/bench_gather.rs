//! Benchmark task gathering and execution.
//!
//! This module reads a benchmark task directory, parses its manifest into a
//! [`TaskManifest`], and executes the benchmark via [`bench_cmd::run_bench`].
//!
//! The canonical manifest is `task.toml`, per the task-contract spec (a task
//! directory holds `task.toml`, `setup.sh`, `verify.sh`, `bench.sh`, and
//! `ref/`). `manifest.json` is accepted as a legacy fallback so task
//! directories written before the contract was pinned keep working; when
//! both are present `task.toml` wins.
//!
//! The module is not yet wired into the CLI; wiring is a separate task.
//! `#![allow(dead_code)]` avoids compiler warnings while uncalled from `main`.

#![allow(dead_code)]

use std::io::Write;
use std::path::Path;

use farmerbob_core::task_contract::TaskManifest;

use crate::bench_cmd::{self, BenchPlan, Outcome};

/// Why a task directory could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatherError {
    /// manifest.json does not exist or could not be read. Carries a non-empty reason.
    Unreadable(String),
    /// manifest.json is not a TaskManifest. Carries a non-empty reason.
    Malformed(String),
}

/// Read a task directory into a manifest.
///
/// Prefers `task.toml` (the task-contract shape); falls back to legacy
/// `manifest.json` when no `task.toml` is present.
pub fn manifest_in(dir: &Path) -> Result<TaskManifest, GatherError> {
    let toml_path = dir.join("task.toml");
    if toml_path.is_file() {
        let content = std::fs::read_to_string(&toml_path).map_err(|e| {
            GatherError::Unreadable(format!(
                "could not read manifest at {}: {e}",
                toml_path.display()
            ))
        })?;
        if content.trim().is_empty() {
            return Err(GatherError::Malformed(
                "manifest contains only whitespace".to_string(),
            ));
        }
        return TaskManifest::parse(&content).map_err(|e| {
            GatherError::Malformed(format!(
                "invalid TaskManifest in {}: {e}",
                toml_path.display()
            ))
        });
    }

    let manifest_path = dir.join("manifest.json");
    let content = match std::fs::read_to_string(&manifest_path) {
        Ok(c) => c,
        Err(e) => {
            return Err(GatherError::Unreadable(format!(
                "could not read manifest at {}: {e}",
                manifest_path.display()
            )));
        }
    };

    if content.trim().is_empty() {
        return Err(GatherError::Malformed(
            "manifest contains only whitespace".to_string(),
        ));
    }

    match serde_json::from_str::<TaskManifest>(&content) {
        Ok(m) => Ok(m),
        Err(e) => Err(GatherError::Malformed(format!(
            "invalid TaskManifest in {}: {e}",
            manifest_path.display()
        ))),
    }
}

/// Render one outcome for an operator. Never empty, never ends in a newline.
pub fn render(task: &str, o: &Outcome) -> String {
    let prefix = if task.is_empty() {
        String::new()
    } else {
        format!("{task}: ")
    };
    match o {
        Outcome::Measured { samples, .. } => {
            let count = samples.len();
            let count_str = if count == 0 {
                "0 samples (zero)".to_string()
            } else {
                format!("{count} samples")
            };
            format!("{prefix}measured {count_str}")
        }
        Outcome::Incorrect(verify) => {
            format!("{prefix}incorrect: {}", verify.detail)
        }
        Outcome::Specialised(verify) => {
            format!("{prefix}specialised: {}", verify.detail)
        }
        Outcome::ReferenceCall(verify) => {
            format!("{prefix}reference-call: {}", verify.detail)
        }
        Outcome::Clock(verdict) => {
            format!("{prefix}clock: {verdict}")
        }
        Outcome::Instrument(reason) => {
            format!("{prefix}instrument: {reason}")
        }
    }
}

/// Render one outcome as machine JSON: everything `fb kernel-score
/// --bench-json` needs, so a wave pipes bench into scoring without
/// hand-parsing log chatter (bead farmerbob-x81s.15). One object, one
/// line: task, status, samples, the widened verify record, the clock
/// verdict, and io_bytes when a trial reported it. Nulls where absent,
/// never missing keys.
pub fn render_json(task: &str, o: &Outcome) -> String {
    use farmerbob_core::task_contract::VerifyOutput;
    let verify_json = |v: &VerifyOutput| {
        serde_json::json!({
            "correct": v.correct,
            "detail": v.detail,
            "tolerance": v.tolerance,
            "precision": v.precision,
            "trials": v.trials,
            "failed_axis": v.failed_axis,
            "specialised": v.specialised,
            "reference_call": v.reference_call,
            "deterministic": v.deterministic,
        })
    };
    let io_bytes_json = |io_bytes: &Option<u64>| match io_bytes {
        Some(n) => serde_json::json!(n),
        None => serde_json::Value::Null,
    };
    let value = match o {
        Outcome::Measured {
            verify,
            samples,
            io_bytes,
        } => serde_json::json!({
            "task": task,
            "status": "measured",
            "samples": samples,
            "verify": verify_json(verify),
            "clock": serde_json::Value::Null,
            "io_bytes": io_bytes_json(io_bytes),
            "detail": verify.detail,
        }),
        Outcome::Incorrect(verify) => serde_json::json!({
            "task": task,
            "status": "incorrect",
            "samples": [],
            "verify": verify_json(verify),
            "clock": serde_json::Value::Null,
            "io_bytes": serde_json::Value::Null,
            "detail": verify.detail,
        }),
        Outcome::Specialised(verify) => serde_json::json!({
            "task": task,
            "status": "specialised",
            "samples": [],
            "verify": verify_json(verify),
            "clock": serde_json::Value::Null,
            "io_bytes": serde_json::Value::Null,
            "detail": verify.detail,
        }),
        Outcome::ReferenceCall(verify) => serde_json::json!({
            "task": task,
            "status": "reference",
            "samples": [],
            "verify": verify_json(verify),
            "clock": serde_json::Value::Null,
            "io_bytes": serde_json::Value::Null,
            "detail": verify.detail,
        }),
        Outcome::Clock(verdict) => serde_json::json!({
            "task": task,
            "status": "clock",
            "samples": [],
            "verify": serde_json::Value::Null,
            "clock": verdict,
            "io_bytes": serde_json::Value::Null,
            "detail": verdict,
        }),
        Outcome::Instrument(reason) => serde_json::json!({
            "task": task,
            "status": "instrument",
            "samples": [],
            "verify": serde_json::Value::Null,
            "clock": serde_json::Value::Null,
            "io_bytes": serde_json::Value::Null,
            "detail": reason,
        }),
    };
    value.to_string()
}
pub fn run(dir: &Path, p: &BenchPlan, out: &mut dyn Write) -> i32 {
    let manifest = match manifest_in(dir) {
        Ok(m) => m,
        Err(GatherError::Unreadable(reason)) => {
            let _ = writeln!(out, "{}: unreadable: {reason}", dir.display());
            return 4;
        }
        Err(GatherError::Malformed(reason)) => {
            let _ = writeln!(out, "{}: malformed: {reason}", dir.display());
            return 4;
        }
    };

    let effective_plan;
    let plan_ref = if p.dir == dir {
        p
    } else {
        effective_plan = BenchPlan {
            dir: dir.to_path_buf(),
            timeout_s: p.timeout_s,
            max_attempts: p.max_attempts,
            max_bad: p.max_bad,
        };
        &effective_plan
    };

    let outcome = bench_cmd::run_bench(&manifest, plan_ref);
    let text = render(&manifest.name.0, &outcome);
    let _ = writeln!(out, "{text}");

    match outcome {
        Outcome::Measured { .. } => 0,
        Outcome::Incorrect(_) => 1,
        // A reference call outranks every other candidate verdict: the
        // run measured the answer key, not the arm.
        Outcome::ReferenceCall(_) => 6,
        // Its own verdict, distinct from incorrect: the arm memorised, it
        // did not merely fail.
        Outcome::Specialised(_) => 5,
        // Distinct from slow (0) and incorrect (1): the number is not
        // trusted at all, and it must not read as either.
        Outcome::Clock(_) => 3,
        Outcome::Instrument(_) => 4,
    }
}

/// Read, run and render as machine JSON. Same outcomes and exit codes as
/// [`run`]: the JSON is the machine-readable twin of the human line, for
/// piping bench into `fb kernel-score --bench-json` (bead
/// farmerbob-x81s.15).
pub fn run_json(dir: &Path, p: &BenchPlan, out: &mut dyn Write) -> i32 {
    let manifest = match manifest_in(dir) {
        Ok(m) => m,
        Err(GatherError::Unreadable(reason)) => {
            let _ = writeln!(
                out,
                "{}",
                serde_json::json!({
                    "task": dir.display().to_string(),
                    "status": "unreadable",
                    "samples": [],
                    "verify": serde_json::Value::Null,
                    "clock": serde_json::Value::Null,
                    "io_bytes": serde_json::Value::Null,
                    "detail": reason,
                })
            );
            return 4;
        }
        Err(GatherError::Malformed(reason)) => {
            let _ = writeln!(
                out,
                "{}",
                serde_json::json!({
                    "task": dir.display().to_string(),
                    "status": "malformed",
                    "samples": [],
                    "verify": serde_json::Value::Null,
                    "clock": serde_json::Value::Null,
                    "io_bytes": serde_json::Value::Null,
                    "detail": reason,
                })
            );
            return 4;
        }
    };

    let effective_plan;
    let plan_ref = if p.dir == dir {
        p
    } else {
        effective_plan = BenchPlan {
            dir: dir.to_path_buf(),
            timeout_s: p.timeout_s,
            max_attempts: p.max_attempts,
            max_bad: p.max_bad,
        };
        &effective_plan
    };

    let outcome = bench_cmd::run_bench(&manifest, plan_ref);
    let text = render_json(&manifest.name.0, &outcome);
    let _ = writeln!(out, "{text}");

    match outcome {
        Outcome::Measured { .. } => 0,
        Outcome::Incorrect(_) => 1,
        Outcome::ReferenceCall(_) => 6,
        Outcome::Specialised(_) => 5,
        Outcome::Clock(_) => 3,
        Outcome::Instrument(_) => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use farmerbob_core::task_contract::{TaskName, Verification, VerifyOutput};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct ScratchDir {
        path: PathBuf,
    }

    impl ScratchDir {
        fn new(tag: &str) -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "fb-bench-gather-test-{}-{}-{}",
                std::process::id(),
                tag,
                n
            ));
            std::fs::create_dir_all(&path).expect("create scratch directory");
            Self { path }
        }

        fn write_file(&self, name: &str, content: &str) {
            std::fs::write(self.path.join(name), content).expect("write scratch file");
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn plan(dir: &Path, timeout_s: u64, max_attempts: u32, max_bad: u32) -> BenchPlan {
        BenchPlan {
            dir: dir.to_path_buf(),
            timeout_s,
            max_attempts,
            max_bad,
        }
    }

    fn manifest_json(name: &str, min_trials: u32) -> String {
        format!(
            r#"{{
                "name": "{name}",
                "description": "benchmark task description",
                "verification": "Benchmark",
                "timeout_s": 0,
                "exclusive": [],
                "min_trials": {min_trials}
            }}"#
        )
    }

    const OK_VERIFY: &str = "echo '{\"correct\":true,\"detail\":\"ok\"}'\n";

    #[test]
    fn clause_1_valid_manifest_yields_task_manifest_field_for_field() {
        let scratch = ScratchDir::new("c1-valid");
        let json = r#"{
            "name": "matrix_mult",
            "description": "matrix multiplication benchmark",
            "verification": "Benchmark",
            "timeout_s": 45,
            "exclusive": ["gpu0", "pstate"],
            "min_trials": 7
        }"#;
        scratch.write_file("manifest.json", json);

        let manifest = manifest_in(&scratch.path).expect("parse valid manifest");
        assert_eq!(manifest.name, TaskName("matrix_mult".to_string()));
        assert_eq!(
            manifest.description,
            "matrix multiplication benchmark".to_string()
        );
        assert_eq!(manifest.verification, Verification::Benchmark);
        assert_eq!(manifest.timeout_s, 45);
        assert_eq!(
            manifest.exclusive,
            vec!["gpu0".to_string(), "pstate".to_string()]
        );
        assert_eq!(manifest.min_trials, 7);
        // Legacy manifests predate the correctness knobs and parse with
        // the audited upstream defaults rather than failing.
        assert_eq!(manifest.correctness_precision, "fp32");
        assert_eq!(manifest.correctness_trials, 1);
    }

    #[test]
    fn clause_2_missing_manifest_is_unreadable_naming_path() {
        let scratch = ScratchDir::new("c2-missing");
        let err = manifest_in(&scratch.path).expect_err("missing manifest must error");
        match err {
            GatherError::Unreadable(reason) => {
                assert!(!reason.is_empty());
                assert!(reason.contains(&scratch.path.to_string_lossy().to_string()));
            }
            GatherError::Malformed(_) => panic!("expected Unreadable, got Malformed"),
        }
    }

    #[test]
    fn clause_3_invalid_or_wrong_shape_is_malformed_and_differs_from_unreadable() {
        let nonexistent =
            std::env::temp_dir().join(format!("fb-nonexistent-gather-dir-{}", std::process::id()));
        let err_unreadable =
            manifest_in(&nonexistent).expect_err("nonexistent dir must be Unreadable");
        assert!(matches!(err_unreadable, GatherError::Unreadable(_)));

        let scratch_bad_syntax = ScratchDir::new("c3-bad-syntax");
        scratch_bad_syntax.write_file("manifest.json", "{not-valid-json");
        let err_bad_syntax = manifest_in(&scratch_bad_syntax.path)
            .expect_err("invalid json syntax must be Malformed");
        assert!(matches!(err_bad_syntax, GatherError::Malformed(_)));

        let scratch_wrong_shape = ScratchDir::new("c3-wrong-shape");
        scratch_wrong_shape.write_file("manifest.json", "[1, 2, 3]");
        let err_wrong_shape =
            manifest_in(&scratch_wrong_shape.path).expect_err("array shape must be Malformed");
        assert!(matches!(err_wrong_shape, GatherError::Malformed(_)));

        assert_ne!(err_unreadable, err_bad_syntax);
        assert_ne!(err_unreadable, err_wrong_shape);
    }

    #[test]
    fn clause_4_run_measured_exits_0_and_mentions_sample_count() {
        let scratch = ScratchDir::new("c4-measured");
        scratch.write_file("manifest.json", &manifest_json("bench_ok", 3));
        scratch.write_file("verify.sh", OK_VERIFY);
        scratch.write_file("bench.sh", "echo '{\"ms\":12.5}'\n");

        let mut out = Vec::new();
        let code = run(&scratch.path, &plan(&scratch.path, 0, 10, 2), &mut out);
        assert_eq!(code, 0);

        let text = String::from_utf8_lossy(&out);
        assert!(text.contains('3'));
    }

    #[test]
    fn clause_5_run_incorrect_exits_1_and_carries_verification_detail() {
        let scratch = ScratchDir::new("c5-incorrect");
        scratch.write_file("manifest.json", &manifest_json("bench_incorrect", 2));
        scratch.write_file(
            "verify.sh",
            "echo '{\"correct\":false,\"detail\":\"checksum_mismatch_at_byte_128\"}'\n",
        );
        scratch.write_file("bench.sh", "echo '{\"ms\":10}'\n");

        let mut out = Vec::new();
        let code = run(&scratch.path, &plan(&scratch.path, 0, 10, 2), &mut out);
        assert_eq!(code, 1);

        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("checksum_mismatch_at_byte_128"));
    }

    #[test]
    fn clause_6_run_instrument_exits_4_and_carries_reason() {
        let scratch = ScratchDir::new("c6-instrument");
        scratch.write_file("manifest.json", &manifest_json("bench_crash", 2));
        scratch.write_file("verify.sh", "echo 'verify script failed' >&2\nexit 2\n");
        scratch.write_file("bench.sh", "echo '{\"ms\":10}'\n");

        let mut out = Vec::new();
        let code = run(&scratch.path, &plan(&scratch.path, 0, 10, 2), &mut out);
        assert_eq!(code, 4);

        let text = String::from_utf8_lossy(&out);
        assert!(text.to_lowercase().contains("verification"));
    }

    #[test]
    fn clause_7_incorrect_and_instrument_exit_codes_differ_in_one_test() {
        let scratch_inc = ScratchDir::new("c7-incorrect");
        scratch_inc.write_file("manifest.json", &manifest_json("bench_inc", 1));
        scratch_inc.write_file(
            "verify.sh",
            "echo '{\"correct\":false,\"detail\":\"wrong_result\"}'\n",
        );
        scratch_inc.write_file("bench.sh", "echo '{\"ms\":5}'\n");

        let mut out_inc = Vec::new();
        let code_incorrect = run(
            &scratch_inc.path,
            &plan(&scratch_inc.path, 0, 10, 2),
            &mut out_inc,
        );

        let scratch_inst = ScratchDir::new("c7-instrument");
        scratch_inst.write_file("manifest.json", &manifest_json("bench_inst", 1));
        scratch_inst.write_file("verify.sh", "echo fail >&2\nexit 1\n");
        scratch_inst.write_file("bench.sh", "echo '{\"ms\":5}'\n");

        let mut out_inst = Vec::new();
        let code_instrument = run(
            &scratch_inst.path,
            &plan(&scratch_inst.path, 0, 10, 2),
            &mut out_inst,
        );

        assert_eq!(code_incorrect, 1);
        assert_eq!(code_instrument, 4);
        assert_ne!(code_incorrect, code_instrument);
    }

    #[test]
    fn clause_8_gather_error_exits_4_matching_instrument() {
        let scratch_no_manifest = ScratchDir::new("c8-missing-manifest");
        scratch_no_manifest.write_file("verify.sh", OK_VERIFY);
        scratch_no_manifest.write_file("bench.sh", "echo '{\"ms\":5}'\n");

        let mut out_gather = Vec::new();
        let code_gather_err = run(
            &scratch_no_manifest.path,
            &plan(&scratch_no_manifest.path, 0, 10, 2),
            &mut out_gather,
        );

        let scratch_broken = ScratchDir::new("c8-broken-bench");
        scratch_broken.write_file("manifest.json", &manifest_json("bench_broken", 1));
        scratch_broken.write_file("verify.sh", "exit 1\n");
        scratch_broken.write_file("bench.sh", "echo '{\"ms\":5}'\n");

        let mut out_broken = Vec::new();
        let code_instrument = run(
            &scratch_broken.path,
            &plan(&scratch_broken.path, 0, 10, 2),
            &mut out_broken,
        );

        assert_eq!(code_gather_err, 4);
        assert_eq!(code_instrument, 4);
        assert_eq!(code_gather_err, code_instrument);
    }
    #[test]
    fn clause_9_render_never_returns_empty_string_for_all_six_outcomes() {
        let measured = Outcome::Measured {
            verify: VerifyOutput {
                correct: true,
                detail: "ok".to_string(),
                tolerance: 1e-4,
                precision: "fp32".to_string(),
                trials: 1,
                failed_axis: None,
                specialised: false,
                reference_call: false,
                deterministic: None,
            },
            samples: vec![10.2, 11.4],
            io_bytes: Some(1_000_000),
        };
        let r_measured = render("task_m", &measured);
        assert!(!r_measured.is_empty());
        assert!(!r_measured.ends_with('\n'));

        let incorrect = Outcome::Incorrect(VerifyOutput {
            correct: false,
            detail: "mismatch".to_string(),
            tolerance: 1e-4,
            precision: "fp32".to_string(),
            trials: 1,
            failed_axis: Some("values".to_string()),
            specialised: false,
            reference_call: false,
            deterministic: None,
        });
        let r_incorrect = render("task_i", &incorrect);
        assert!(!r_incorrect.is_empty());
        assert!(!r_incorrect.ends_with('\n'));

        let instrument = Outcome::Instrument("timeout exceeded".to_string());
        let r_instrument = render("task_inst", &instrument);
        assert!(!r_instrument.is_empty());
        assert!(!r_instrument.ends_with('\n'));

        // The clock verdict renders named, never empty: a corrupted clock
        // must read as clock-tamper, not as slow or incorrect.
        let clock = Outcome::Clock("tampered:time.perf_counter: rebound".to_string());
        let r_clock = render("task_c", &clock);
        assert!(r_clock.contains("clock"), "{r_clock}");
        assert!(r_clock.contains("tampered"), "{r_clock}");
        assert!(!r_clock.ends_with('\n'));

        // Specialisation renders under its own name, never as incorrect.
        let specialised = Outcome::Specialised(VerifyOutput {
            correct: false,
            detail: "heldout".to_string(),
            tolerance: 1e-4,
            precision: "fp32".to_string(),
            trials: 1,
            failed_axis: Some("specialised".to_string()),
            specialised: true,
            reference_call: false,
            deterministic: None,
        });
        let r_specialised = render("task_s", &specialised);
        assert!(r_specialised.contains("specialised"), "{r_specialised}");
        assert!(!r_specialised.ends_with('\n'));

        // A reference call renders under its own name, never as
        // incorrect and never as slow.
        let reference = Outcome::ReferenceCall(VerifyOutput {
            correct: false,
            detail: "ref".to_string(),
            tolerance: 1e-4,
            precision: "fp32".to_string(),
            trials: 1,
            failed_axis: None,
            specialised: false,
            reference_call: true,
            deterministic: None,
        });
        let r_reference = render("task_r", &reference);
        assert!(r_reference.contains("reference-call"), "{r_reference}");
        assert!(!r_reference.ends_with('\n'));
    }

    /// A clock verdict exits 3: distinct from measured (0), incorrect (1)
    /// and instrument (4). Pinned end to end through real scripts.
    #[test]
    fn boundary_clock_verdict_exits_3_and_names_itself() {
        let scratch = ScratchDir::new("b-clock");
        scratch.write_file("manifest.json", &manifest_json("bench_clock", 1));
        scratch.write_file("verify.sh", OK_VERIFY);
        scratch.write_file(
            "bench.sh",
            "echo '{\"clock\":\"tampered:torch.cuda.Event\",\"detail\":\"rebound\"}'\nexit 1\n",
        );

        let mut out = Vec::new();
        let code = run(&scratch.path, &plan(&scratch.path, 0, 10, 5), &mut out);
        assert_eq!(code, 3);

        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("clock"), "{text}");
        assert!(text.contains("tampered"), "{text}");
    }

    /// A specialised verification exits 5 with its own name: distinct from
    /// incorrect (1). Pinned end to end through real scripts (bead
    /// farmerbob-x81s.4).
    #[test]
    fn boundary_specialised_verification_exits_5_and_names_itself() {
        let scratch = ScratchDir::new("b-specialised");
        scratch.write_file("manifest.json", &manifest_json("bench_spec", 1));
        scratch.write_file(
            "verify.sh",
            "echo '{\"correct\":false,\"detail\":\"heldout\",\"specialised\":true}'\n",
        );
        scratch.write_file("bench.sh", "touch marker_must_not_run\n");

        let mut out = Vec::new();
        let code = run(&scratch.path, &plan(&scratch.path, 0, 10, 2), &mut out);
        assert_eq!(code, 5);

        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("specialised"), "{text}");
        assert!(!scratch.path.join("marker_must_not_run").exists());
    }

    /// A reference call exits 6 with its own name: distinct from
    /// incorrect (1), slow (0) and specialised (5). Pinned end to end
    /// through real scripts (bead farmerbob-x81s.5).
    #[test]
    fn boundary_reference_call_exits_6_and_names_itself() {
        let scratch = ScratchDir::new("b-reference");
        scratch.write_file("manifest.json", &manifest_json("bench_ref", 1));
        scratch.write_file(
            "verify.sh",
            "echo '{\"correct\":false,\"detail\":\"ref\",\"reference_call\":true}'\n",
        );
        scratch.write_file("bench.sh", "touch marker_must_not_run\n");

        let mut out = Vec::new();
        let code = run(&scratch.path, &plan(&scratch.path, 0, 10, 2), &mut out);
        assert_eq!(code, 6);

        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("reference-call"), "{text}");
        assert!(!scratch.path.join("marker_must_not_run").exists());
    }

    /// The JSON twin carries everything scoring needs: samples, the
    /// widened verify record, and io_bytes when a trial reported it.
    #[test]
    fn json_twin_carries_samples_verify_and_io_bytes() {
        let scratch = ScratchDir::new("b-json");
        scratch.write_file("manifest.json", &manifest_json("bench_json", 1));
        scratch.write_file("verify.sh", OK_VERIFY);
        scratch.write_file(
            "bench.sh",
            "echo '{\"ms\":2.0,\"metrics\":{\"io_bytes\":800}}'\n",
        );

        let mut out = Vec::new();
        let code = run_json(&scratch.path, &plan(&scratch.path, 0, 10, 5), &mut out);
        assert_eq!(code, 0);
        let value: serde_json::Value =
            serde_json::from_str(&String::from_utf8_lossy(&out)).expect("one JSON object");
        assert_eq!(value["status"], "measured");
        assert_eq!(value["samples"], serde_json::json!([2.0]));
        assert_eq!(value["verify"]["correct"], true);
        assert_eq!(value["io_bytes"], 800);
        assert_eq!(value["task"], "bench_json");
    }

    /// Non-measured outcomes JSON-ify with empty samples and named
    /// details, same exit codes as the human path.
    #[test]
    fn json_twin_names_non_measured_outcomes() {
        let scratch = ScratchDir::new("b-json-inc");
        scratch.write_file("manifest.json", &manifest_json("bench_json_inc", 1));
        scratch.write_file(
            "verify.sh",
            "echo '{\"correct\":false,\"detail\":\"mismatch\",\"failed_axis\":\"values\"}'\n",
        );
        scratch.write_file("bench.sh", "touch marker_must_not_run\n");

        let mut out = Vec::new();
        let code = run_json(&scratch.path, &plan(&scratch.path, 0, 10, 2), &mut out);
        assert_eq!(code, 1);
        let value: serde_json::Value =
            serde_json::from_str(&String::from_utf8_lossy(&out)).expect("one JSON object");
        assert_eq!(value["status"], "incorrect");
        assert_eq!(value["samples"], serde_json::json!([]));
        assert_eq!(value["verify"]["failed_axis"], "values");
        assert!(!scratch.path.join("marker_must_not_run").exists());
    }

    #[test]
    fn boundary_measured_zero_samples_exits_0_and_says_zero() {
        let scratch = ScratchDir::new("b-zero-samples");
        scratch.write_file("manifest.json", &manifest_json("bench_zero", 0));
        scratch.write_file("verify.sh", OK_VERIFY);
        scratch.write_file("bench.sh", "touch marker_must_not_run\n");

        let mut out = Vec::new();
        let code = run(&scratch.path, &plan(&scratch.path, 0, 10, 2), &mut out);
        assert_eq!(code, 0);

        let text = String::from_utf8_lossy(&out);
        assert!(text.contains('0') || text.to_lowercase().contains("zero"));
        assert!(!scratch.path.join("marker_must_not_run").exists());
    }

    #[test]
    fn boundary_empty_json_object_is_malformed() {
        let scratch = ScratchDir::new("b-empty-obj");
        scratch.write_file("manifest.json", "{}");

        let err = manifest_in(&scratch.path).expect_err("empty json object must be Malformed");
        assert!(matches!(err, GatherError::Malformed(_)));
    }

    #[test]
    fn boundary_directory_does_not_exist_is_unreadable_not_panic() {
        let nonexistent = std::env::temp_dir().join(format!(
            "fb-bench-gather-missing-dir-{}-{}",
            std::process::id(),
            9999
        ));
        let err = manifest_in(&nonexistent).expect_err("nonexistent dir must be Unreadable");
        assert!(matches!(err, GatherError::Unreadable(_)));
    }

    #[test]
    fn composition_gather_error_short_circuits_without_spawning_scripts() {
        let scratch = ScratchDir::new("comp-short-circuit");
        // verify.sh creates a marker file if spawned
        scratch.write_file("verify.sh", "touch verify_ran_marker\nexit 0\n");
        scratch.write_file("bench.sh", "touch bench_ran_marker\nexit 0\n");
        // Note: NO manifest.json is created

        let mut out = Vec::new();
        let code = run(&scratch.path, &plan(&scratch.path, 0, 10, 2), &mut out);
        assert_eq!(code, 4);
        assert!(!scratch.path.join("verify_ran_marker").exists());
        assert!(!scratch.path.join("bench_ran_marker").exists());
    }

    fn task_toml(name: &str, min_trials: u32) -> String {
        format!(
            "name = \"{name}\"\ndescription = \"benchmark task\"\nverification = \"Benchmark\"\nmin_trials = {min_trials}\n"
        )
    }

    #[test]
    fn task_toml_is_the_canonical_manifest() {
        let scratch = ScratchDir::new("toml-canonical");
        scratch.write_file("task.toml", &task_toml("toml_task", 2));
        let manifest = manifest_in(&scratch.path).expect("task.toml must parse");
        assert_eq!(manifest.name.0, "toml_task");
        assert_eq!(manifest.min_trials, 2);
    }

    #[test]
    fn task_toml_wins_over_legacy_manifest_json() {
        let scratch = ScratchDir::new("toml-wins");
        scratch.write_file("task.toml", &task_toml("from_toml", 1));
        scratch.write_file("manifest.json", &manifest_json("from_json", 1));
        let manifest = manifest_in(&scratch.path).expect("both manifests present");
        assert_eq!(manifest.name.0, "from_toml");
    }

    #[test]
    fn legacy_manifest_json_still_works_alone() {
        let scratch = ScratchDir::new("json-legacy");
        scratch.write_file("manifest.json", &manifest_json("legacy_task", 1));
        let manifest = manifest_in(&scratch.path).expect("legacy manifest must parse");
        assert_eq!(manifest.name.0, "legacy_task");
    }

    #[test]
    fn invalid_task_toml_is_malformed() {
        let scratch = ScratchDir::new("toml-invalid");
        scratch.write_file("task.toml", "name = \"x\"\nverification = \"Benchmark\"\n");
        let err = manifest_in(&scratch.path).expect_err("min_trials 0 must be Malformed");
        assert!(matches!(err, GatherError::Malformed(_)));
    }

    #[test]
    fn task_toml_run_measures_end_to_end() {
        let scratch = ScratchDir::new("toml-run");
        scratch.write_file("task.toml", &task_toml("toml_run", 1));
        scratch.write_file("verify.sh", OK_VERIFY);
        scratch.write_file("bench.sh", "echo '{\"ms\":7.5}'\n");
        let mut out = Vec::new();
        let code = run(&scratch.path, &plan(&scratch.path, 0, 10, 2), &mut out);
        assert_eq!(code, 0);
        assert!(String::from_utf8_lossy(&out).contains("toml_run"));
    }
}
