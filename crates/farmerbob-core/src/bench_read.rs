//! Read verification and benchmark values from captured script output.

use crate::task_contract::{BenchOutput, TaskManifest, VerifyOutput};
use serde_json::Value;

/// One trial's raw stdout, and whether the process exited zero.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    /// The captured standard output.
    pub stdout: String,
    /// Whether the process exited successfully.
    pub exited_zero: bool,
}

/// Why a run could not be read.
#[derive(Debug, Clone, PartialEq)]
pub enum ReadError {
    /// The process exited non-zero. Carries the stdout, which usually says why.
    Failed(String),
    /// It exited zero and printed nothing parseable. Carries what it printed.
    Unparseable(String),
    /// It printed a number that cannot be a measurement. Carries the offending text.
    NotAMeasurement(String),
}

/// Read one verification run.
pub fn verify(r: &Run) -> Result<VerifyOutput, ReadError> {
    if !r.exited_zero {
        return Err(ReadError::Failed(r.stdout.clone()));
    }

    let Some(value) = last_object_with_field(&r.stdout, "correct") else {
        return Err(ReadError::Unparseable(r.stdout.clone()));
    };
    let Value::Object(object) = value else {
        return Err(ReadError::Unparseable(r.stdout.clone()));
    };
    let Some(correct) = object.get("correct").and_then(Value::as_bool) else {
        return Err(ReadError::Unparseable(r.stdout.clone()));
    };
    let Some(detail) = object.get("detail").and_then(Value::as_str) else {
        return Err(ReadError::Unparseable(r.stdout.clone()));
    };
    // Widened verdict fields (bead farmerbob-x81s.3): what the comparison
    // enforced, so a stored result can be re-checked later. Absent in
    // outputs written before the harness recorded them.
    let tolerance = object
        .get("tolerance")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let precision = object
        .get("precision")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let trials = object
        .get("trials")
        .and_then(Value::as_u64)
        .map(|t| t.min(u32::MAX as u64) as u32)
        .unwrap_or(0);
    let failed_axis = object
        .get("failed_axis")
        .and_then(Value::as_str)
        .map(str::to_string);
    // Held-out verdict (bead farmerbob-x81s.4) and reference-call answer
    // (bead farmerbob-x81s.5): absent in outputs written before the
    // harness checked them.
    let specialised = object
        .get("specialised")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let reference_call = object
        .get("reference_call")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    // Bitwise determinism observation (bead farmerbob-x81s.7): true or
    // false when the check ran, absent otherwise.
    let deterministic = object.get("deterministic").and_then(Value::as_bool);

    Ok(VerifyOutput {
        correct,
        detail: detail.to_string(),
        tolerance,
        precision,
        trials,
        failed_axis,
        specialised,
        reference_call,
        deterministic,
    })
}

/// Read one benchmark run.
pub fn bench(r: &Run) -> Result<BenchOutput, ReadError> {
    if !r.exited_zero {
        return Err(ReadError::Failed(r.stdout.clone()));
    }

    let Some(value) = last_object_with_field(&r.stdout, "ms") else {
        if let Some(offending) = special_non_finite_measurement(&r.stdout) {
            return Err(ReadError::NotAMeasurement(offending));
        }
        return Err(ReadError::Unparseable(r.stdout.clone()));
    };
    let Value::Object(object) = value else {
        return Err(ReadError::Unparseable(r.stdout.clone()));
    };
    let Some(ms_value) = object.get("ms") else {
        return Err(ReadError::Unparseable(r.stdout.clone()));
    };
    let Some(ms) = ms_value.as_f64() else {
        return Err(ReadError::Unparseable(r.stdout.clone()));
    };
    if !ms.is_finite() || ms < 0.0 {
        return Err(ReadError::NotAMeasurement(ms_value.to_string()));
    }

    Ok(BenchOutput {
        ms,
        metrics: object.get("metrics").cloned().unwrap_or(Value::Null),
    })
}

/// Read a clock verdict from any run's stdout, regardless of exit code.///
/// A clock-tampered trial prints `{"clock": "tampered:..."/"divergent:..."`}
/// and exits non-zero with NO `ms`, so [`bench`] never sees it -- it
/// rejects non-zero exits first. Without this reader a corrupted clock
/// would read as an ordinary bad trial, which is exactly the conflation
/// bead farmerbob-x81s.2 exists to prevent. Returns `None` for a clean
/// (`"clock": "ok"`) or absent verdict.
pub fn clock_verdict(r: &Run) -> Option<String> {
    let value = last_object_with_field(&r.stdout, "clock")?;
    let Value::Object(object) = value else {
        return None;
    };
    let clock = object.get("clock").and_then(Value::as_str)?;
    if clock == "ok" {
        return None;
    }
    match object.get("detail").and_then(Value::as_str) {
        Some(detail) if !detail.is_empty() => Some(format!("{clock}: {detail}")),
        _ => Some(clock.to_string()),
    }
}

/// Read a whole trial set, keeping every failure.
pub fn samples(runs: &[Run]) -> (Vec<f64>, Vec<(usize, ReadError)>) {
    let mut successful = Vec::new();
    let mut failures = Vec::new();

    for (index, run) in runs.iter().enumerate() {
        match bench(run) {
            Ok(output) => successful.push(output.ms),
            Err(error) => failures.push((index, error)),
        }
    }

    (successful, failures)
}

/// Read the reported memory traffic from one benchmark output: the
/// `io_bytes` the bench shim records in trial metrics (bead
/// farmerbob-x81s.15). Every trial of one task measures the same tensors,
/// so the first present value wins upstream.
pub fn io_bytes(output: &BenchOutput) -> Option<u64> {
    output.metrics.get("io_bytes")?.as_u64()
}

/// Whether enough trials survived to score, per the manifest.
pub fn enough(m: &TaskManifest, samples: &[f64]) -> bool {
    samples.len() >= m.min_trials as usize
}

fn last_object_with_field(stdout: &str, field: &str) -> Option<Value> {
    let mut result = None;
    for line in stdout.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let Value::Object(object) = value else {
            continue;
        };
        if object.contains_key(field) {
            result = Some(Value::Object(object));
        }
    }
    result
}

fn special_non_finite_measurement(stdout: &str) -> Option<String> {
    let mut result = None;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if !(trimmed.starts_with('{') && trimmed.ends_with('}')) {
            continue;
        }

        let mut search_from = 0;
        while let Some(relative) = trimmed[search_from..].find("\"ms\"") {
            let key_start = search_from + relative;
            let after_key = key_start + "\"ms\"".len();
            let Some(colon) = trimmed[after_key..].find(':') else {
                break;
            };
            let value_start = after_key + colon + 1;
            let value_start = trimmed[value_start..]
                .char_indices()
                .find(|(_, character)| !character.is_whitespace())
                .map(|(offset, _)| value_start + offset);
            let Some(value_start) = value_start else {
                break;
            };

            let value_end = trimmed[value_start..]
                .char_indices()
                .find(|(_, character)| {
                    character.is_whitespace() || *character == ',' || *character == '}'
                })
                .map(|(offset, _)| value_start + offset)
                .unwrap_or(trimmed.len());
            let token = &trimmed[value_start..value_end];
            if !token.starts_with('"') && is_non_finite_number(token) {
                result = Some(token.to_string());
            }
            search_from = value_end;
        }
    }
    result
}

fn is_non_finite_number(token: &str) -> bool {
    if let Ok(value) = token.parse::<f64>() {
        return !value.is_finite();
    }

    matches!(
        token.to_ascii_lowercase().as_str(),
        "nan" | "+nan" | "-nan" | "inf" | "+inf" | "-inf" | "infinity" | "+infinity" | "-infinity"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_contract::{TaskName, Verification};

    fn run(stdout: &str) -> Run {
        Run {
            stdout: stdout.to_string(),
            exited_zero: true,
        }
    }

    fn manifest(min_trials: u32) -> TaskManifest {
        TaskManifest {
            name: TaskName("bench".to_string()),
            description: String::new(),
            verification: Verification::Benchmark,
            timeout_s: 0,
            exclusive: Vec::new(),
            min_trials,
            correctness_precision: "fp32".to_string(),
            correctness_trials: 1,
            determinism: "required".to_string(),
        }
    }

    #[test]
    fn verification_failure_is_a_successful_read_but_failed_process_is_not() {
        let stdout = r#"{"correct":false,"detail":"mismatch at 3"}"#;
        let verification = verify(&Run {
            stdout: stdout.to_string(),
            exited_zero: true,
        });
        assert_eq!(
            verification,
            Ok(VerifyOutput {
                correct: false,
                detail: "mismatch at 3".to_string(),
                tolerance: 0.0,
                precision: String::new(),
                trials: 0,
                failed_axis: None,
                specialised: false,
                reference_call: false,
                deterministic: None,
            })
        );

        let failed = verify(&Run {
            stdout: stdout.to_string(),
            exited_zero: false,
        });
        assert_eq!(failed, Err(ReadError::Failed(stdout.to_string())));
    }

    #[test]
    fn benchmark_reads_last_object_and_defaults_metrics_to_null() {
        let output = bench(&run(
            "progress\n{\"ms\": 8.0}\n{\"ms\": 12.5,\"metrics\":null}\ncomplete",
        ));
        assert_eq!(
            output,
            Ok(BenchOutput {
                ms: 12.5,
                metrics: Value::Null,
            })
        );
    }

    #[test]
    fn non_measurements_are_rejected_and_zero_is_allowed() {
        assert!(matches!(
            bench(&run(r#"{"ms":-1}"#)),
            Err(ReadError::NotAMeasurement(_))
        ));
        assert!(matches!(
            bench(&run(r#"{"ms":NaN}"#)),
            Err(ReadError::NotAMeasurement(_))
        ));
        assert!(matches!(
            bench(&run(r#"{"ms":Infinity}"#)),
            Err(ReadError::NotAMeasurement(_))
        ));
        assert_eq!(
            bench(&run(r#"{"ms":0.0}"#)).map(|output| output.ms),
            Ok(0.0)
        );
    }

    #[test]
    fn samples_keep_order_and_index_every_failure() {
        let runs = vec![
            run(r#"{"ms":1.0}"#),
            Run {
                stdout: "failed".to_string(),
                exited_zero: false,
            },
            run(r#"{"ms":"fast"}"#),
            run(r#"{"ms":-1}"#),
        ];

        let (successful, failures) = samples(&runs);
        assert_eq!(successful, vec![1.0]);
        assert_eq!(failures.len(), 3);
        assert_eq!(failures[0].0, 1);
        assert_eq!(failures[1].0, 2);
        assert_eq!(failures[2].0, 3);
        assert!(matches!(failures[0].1, ReadError::Failed(_)));
        assert!(matches!(failures[1].1, ReadError::Unparseable(_)));
        assert!(matches!(failures[2].1, ReadError::NotAMeasurement(_)));
    }

    #[test]
    fn enough_uses_only_sample_count_including_zero_minimum() {
        let no_samples = Vec::new();
        assert!(enough(&manifest(0), &no_samples));
        assert!(!enough(&manifest(1), &no_samples));
        assert!(enough(&manifest(2), &[1.0, 2.0]));
        assert!(!enough(&manifest(3), &[1.0, 2.0]));
    }

    /// A clean or absent clock verdict is None, whatever the exit code: only
    /// a non-ok verdict names anything.
    #[test]
    fn clock_ok_and_absent_are_silence() {
        assert_eq!(clock_verdict(&run(r#"{"ms":1.0,"clock":"ok"}"#)), None);
        assert_eq!(clock_verdict(&run(r#"{"ms":1.0}"#)), None);
        assert_eq!(
            clock_verdict(&Run {
                stdout: "crashed".to_string(),
                exited_zero: false,
            }),
            None
        );
    }

    /// A tampered trial names its verdict even though it exits non-zero --
    /// the case `bench` can never see, which is why this reader exists.
    #[test]
    fn clock_tamper_survives_a_nonzero_exit() {
        let verdict = clock_verdict(&Run {
            stdout: "[Profiling] chatter\n{\"clock\":\"tampered:time.perf_counter\",\"detail\":\"timing primitive rebound during eval\"}".to_string(),
            exited_zero: false,
        });
        assert_eq!(
            verdict,
            Some("tampered:time.perf_counter: timing primitive rebound during eval".to_string())
        );
    }

    /// Divergence verdicts read the same way, detail included.
    #[test]
    fn clock_divergence_reads_with_detail() {
        let verdict = clock_verdict(&Run {
            stdout: "{\"clock\":\"divergent:reported=0.001ms x 10 trials > wall=12.40s\",\"detail\":\"reported kernel time exceeds harness wall time\"}".to_string(),
            exited_zero: false,
        });
        let Some(v) = verdict else {
            panic!("expected a verdict");
        };
        assert!(v.starts_with("divergent:"), "{v}");
        assert!(v.contains("wall"), "{v}");
    }

    /// Widened verdicts carry what the comparison enforced, so a stored
    /// result can be re-checked later (bead farmerbob-x81s.3).
    #[test]
    fn widened_verdict_carries_tolerance_policy_trials_and_axis() {
        let stdout = r#"{"correct":false,"detail":"values differ (max 0.003)","clock":"ok","tolerance":0.0001,"precision":"fp32","trials":1,"failed_axis":"values"}"#;
        assert_eq!(
            verify(&run(stdout)),
            Ok(VerifyOutput {
                correct: false,
                detail: "values differ (max 0.003)".to_string(),
                tolerance: 1e-4,
                precision: "fp32".to_string(),
                trials: 1,
                failed_axis: Some("values".to_string()),
                specialised: false,
                reference_call: false,
                deterministic: None,
            })
        );
    }

    /// The held-out verdict parses with it: specialisation rides alongside
    /// the widened fields, defaulting false for old outputs (bead
    /// farmerbob-x81s.4).
    #[test]
    fn specialised_verdict_parses_and_defaults_false() {
        let stdout = r#"{"correct":false,"detail":"heldout seed=1: SPECIALISED","clock":"ok","tolerance":0.0001,"precision":"fp32","trials":1,"failed_axis":"specialised","specialised":true}"#;
        let v = verify(&run(stdout)).expect("parse");
        assert!(!v.correct);
        assert!(v.specialised);
        assert_eq!(v.failed_axis, Some("specialised".to_string()));
        let old = verify(&run(r#"{"correct":true,"detail":"ok"}"#)).expect("parse");
        assert!(!old.specialised);
        assert!(!old.reference_call);
    }

    /// The reference-call answer parses with it, defaulting false for old
    /// outputs (bead farmerbob-x81s.5).
    #[test]
    fn reference_call_parses_and_defaults_false() {
        let stdout = r#"{"correct":false,"detail":"ref","clock":"ok","tolerance":0.0001,"precision":"fp32","trials":1,"failed_axis":null,"specialised":false,"reference_call":true,"runtime_modules":[["m","/t/ref/problem.py"]]}"#;
        let v = verify(&run(stdout)).expect("parse");
        assert!(!v.correct);
        assert!(v.reference_call);
        assert!(!v.specialised);
    }

    /// io_bytes reads from trial metrics, absent when unreported (bead
    /// farmerbob-x81s.15).
    #[test]
    fn io_bytes_reads_metrics() {
        let with = bench(&run(r#"{"ms":5,"metrics":{"io_bytes":800}}"#)).expect("parse");
        assert_eq!(io_bytes(&with), Some(800));
        let without = bench(&run(r#"{"ms":5}"#)).expect("parse");
        assert_eq!(io_bytes(&without), None);
    }

    /// Determinism observations parse true/false/absent (bead
    /// farmerbob-x81s.7).
    #[test]
    fn deterministic_parses_tri_state() {
        let yes = verify(&run(
            r#"{"correct":true,"detail":"ok","deterministic":true}"#,
        ))
        .expect("parse");
        assert_eq!(yes.deterministic, Some(true));
        let no = verify(&run(
            r#"{"correct":false,"detail":"x","deterministic":false}"#,
        ))
        .expect("parse");
        assert_eq!(no.deterministic, Some(false));
        let old = verify(&run(r#"{"correct":true,"detail":"ok"}"#)).expect("parse");
        assert_eq!(old.deterministic, None);
    }
}
