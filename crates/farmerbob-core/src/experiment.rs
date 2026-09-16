//! Experiments: benchmark commands a run executes and their measured results.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::{ExperimentId, RunId};

/// One benchmark command executed within a run, plus its measured outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Experiment {
    pub id: ExperimentId,
    pub run_id: RunId,
    /// The command line that was executed.
    pub command: String,
    /// Free-form measurements reported by the benchmark (accuracy, tokens,
    /// timings, ...). Kept as raw JSON so graders define their own schema.
    pub metrics: Value,
    /// Whether the experiment's output was judged correct, if it was judged.
    pub correct: Option<bool>,
    /// How long the command took, in milliseconds, if it was measured.
    pub duration_ms: Option<u64>,
    /// True when the result was flagged as an outlier and held out of
    /// aggregate statistics.
    pub quarantined: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn experiment_round_trips_through_json() {
        let experiment = Experiment {
            id: ExperimentId::new(),
            run_id: RunId::new(),
            command: String::from("python -m bench.run --suite kernelbench"),
            metrics: serde_json::json!({
                "accuracy": 0.87,
                "tokens_out": 41_233,
                "per_problem": [{"name": "k1", "passed": true}]
            }),
            correct: Some(true),
            duration_ms: Some(12_345),
            quarantined: false,
        };

        let back: Experiment =
            serde_json::from_str(&serde_json::to_string(&experiment).unwrap()).unwrap();
        assert_eq!(experiment, back);
    }
}
