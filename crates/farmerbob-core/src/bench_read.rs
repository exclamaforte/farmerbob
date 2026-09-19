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

    Ok(VerifyOutput {
        correct,
        detail: detail.to_string(),
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
}
