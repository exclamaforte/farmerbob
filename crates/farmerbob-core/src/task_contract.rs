//! Parsing and scoring for experiment task manifests and script output.

/// A task's stable name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct TaskName(pub String);

/// What kind of evidence a task's success rests on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Verification {
    /// Success is established by deterministic tests.
    DeterministicTests,
    /// Success is established by benchmark measurements.
    Benchmark,
    /// Success is established by a reviewer.
    ReviewerJudgement,
}

/// The manifest describing one task.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TaskManifest {
    /// The task name.
    pub name: TaskName,
    /// Human readable task description.
    pub description: String,
    /// Evidence used to verify success.
    pub verification: Verification,
    /// Seconds. Zero or absent means no limit.
    pub timeout_s: u64,
    /// Named resources this task needs held exclusively.
    pub exclusive: Vec<String>,
    /// Minimum measured trials before a benchmark result may be scored.
    pub min_trials: u32,
}

/// The output printed by `bench.sh`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BenchOutput {
    /// Measured milliseconds.
    pub ms: f64,
    /// Additional benchmark metrics.
    pub metrics: serde_json::Value,
}

/// The output printed by `verify.sh`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VerifyOutput {
    /// Whether verification succeeded.
    pub correct: bool,
    /// Human readable verification detail.
    pub detail: String,
}

/// The score assigned to an experiment.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Score {
    /// Correct and measured.
    Scored { speedup: f64, trials: u32 },
    /// Ran, but produced an incorrect result.
    Incorrect { detail: String },
    /// Measured but not trustworthy.
    Unreliable { reason: String },
    /// Could not be measured.
    NotMeasured { reason: String },
}

impl TaskManifest {
    /// Parse a task manifest from a small TOML document.
    pub fn parse(toml_src: &str) -> Result<TaskManifest, String> {
        let mut name = None;
        let mut description = None;
        let mut verification = None;
        let mut timeout_s = 0;
        let mut exclusive = Vec::new();
        let mut min_trials = 0;
        for raw in toml_src.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err("invalid manifest line".to_string());
            };
            let key = key.trim();
            let value = strip_comment(value.trim());
            match key {
                "name" => name = Some(TaskName(parse_string(value)?)),
                "description" => description = Some(parse_string(value)?),
                "verification" => {
                    let v = parse_string(value)?;
                    verification = Some(match v.as_str() {
                        "DeterministicTests" => Verification::DeterministicTests,
                        "Benchmark" => Verification::Benchmark,
                        "ReviewerJudgement" => Verification::ReviewerJudgement,
                        _ => return Err("invalid verification".to_string()),
                    });
                }
                "timeout_s" => timeout_s = value.parse().map_err(|_| "invalid timeout_s".to_string())?,
                "min_trials" => min_trials = value.parse().map_err(|_| "invalid min_trials".to_string())?,
                "exclusive" => exclusive = parse_array(value)?,
                _ => {}
            }
        }
        let name = name.ok_or_else(|| "missing name".to_string())?;
        if name.0.trim().is_empty() {
            return Err("name cannot be empty".to_string());
        }
        let verification = verification.ok_or_else(|| "missing verification".to_string())?;
        if verification == Verification::Benchmark && min_trials == 0 {
            return Err("benchmark requires min_trials".to_string());
        }
        Ok(TaskManifest {
            name,
            description: description.unwrap_or_default(),
            verification,
            timeout_s,
            exclusive,
            min_trials,
        })
    }
}

fn strip_comment(value: &str) -> &str {
    let mut quoted = false;
    for (i, c) in value.char_indices() {
        if c == '"' { quoted = !quoted; }
        if c == '#' && !quoted { return value[..i].trim(); }
    }
    value
}

fn parse_string(value: &str) -> Result<String, String> {
    let v = value.trim();
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        Ok(v[1..v.len() - 1].replace("\\\"", "\"").replace("\\\\", "\\"))
    } else { Err("expected quoted string".to_string()) }
}

fn parse_array(value: &str) -> Result<Vec<String>, String> {
    let v = value.trim();
    if !(v.starts_with('[') && v.ends_with(']')) { return Err("expected array".to_string()); }
    let inner = v[1..v.len() - 1].trim();
    if inner.is_empty() { return Ok(Vec::new()); }
    inner.split(',').map(|s| parse_string(s.trim())).collect()
}

/// Score one experiment according to correctness, sample count, validity, and spread.
pub fn score(manifest: &TaskManifest, verify: &VerifyOutput, samples: &[f64], baseline_ms: f64, max_spread: f64) -> Score {
    if !verify.correct { return Score::Incorrect { detail: verify.detail.clone() }; }
    if samples.len() < manifest.min_trials as usize {
        return Score::Unreliable { reason: "too few trials".to_string() };
    }
    if !baseline_ms.is_finite() || samples.iter().any(|s| !s.is_finite() || *s <= 0.0) {
        return Score::NotMeasured { reason: "non-positive or non-finite measurement".to_string() };
    }
    let Some(med) = median(samples) else {
        return Score::NotMeasured { reason: "no measurements".to_string() };
    };
    let (min, max) = samples.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &x| (lo.min(x), hi.max(x)));
    if (max - min) / med > max_spread {
        return Score::Unreliable { reason: "sample spread exceeds threshold".to_string() };
    }
    Score::Scored { speedup: baseline_ms / med, trials: samples.len() as u32 }
}

/// Return the median of a slice without modifying it.
pub fn median(samples: &[f64]) -> Option<f64> {
    if samples.is_empty() { return None; }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) { Some((sorted[mid - 1] + sorted[mid]) / 2.0) } else { Some(sorted[mid]) }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn manifest(min_trials: u32) -> TaskManifest {
        TaskManifest { name: TaskName("x".into()), description: String::new(), verification: Verification::Benchmark, timeout_s: 0, exclusive: vec![], min_trials }
    }
    #[test] fn incorrect_fast_is_incorrect() { assert_eq!(score(&manifest(1), &VerifyOutput { correct: false, detail: "bad".into() }, &[1.0], 100.0, 1.0), Score::Incorrect { detail: "bad".into() }); }
    #[test] fn too_few_trials_is_unreliable() { assert!(matches!(score(&manifest(2), &VerifyOutput { correct: true, detail: String::new() }, &[1.0], 100.0, 1.0), Score::Unreliable { .. })); }
    #[test] fn invalid_sample_is_not_measured() { assert!(matches!(score(&manifest(1), &VerifyOutput { correct: true, detail: String::new() }, &[0.0], 100.0, 1.0), Score::NotMeasured { .. })); }
    #[test] fn high_spread_is_unreliable() { assert!(matches!(score(&manifest(2), &VerifyOutput { correct: true, detail: String::new() }, &[1.0, 10.0], 100.0, 0.5), Score::Unreliable { .. })); }
    #[test] fn median_even_and_odd() { assert_eq!(median(&[3.0, 1.0, 2.0, 4.0]), Some(2.5)); assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0)); }
    #[test] fn benchmark_zero_trials_rejected() { let src = "name = \"x\"\nverification = \"Benchmark\"\nmin_trials = 0"; assert!(TaskManifest::parse(src).is_err()); }
}
