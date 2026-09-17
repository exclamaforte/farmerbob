//! Ingest the bootstrap harness's run records into the store.
//!
//! The shell harness that built farmerbob writes one JSON blob per run. Those blobs are the
//! project's entire history — 65 runs across 24 arms — and they are the seed data the router
//! needs. This turns them into rows.
//!
//! The load-bearing part is [`OutcomeClass`]. Of eleven no-ops in one early round, **nine were
//! not arm failures**: a task whose premise a merge had invalidated, an adapter that could not
//! do multi-turn work, an upstream rate limit caused by our own dispatch burst, a stale
//! adapter config, a harness bug, and a run the orchestrator itself killed for cost. Importing
//! those as failures would teach the router to avoid arms for things it did to them.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Why a run ended, and therefore whether it says anything about the arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeClass {
    /// The arm attempted the task and the result reflects its work.
    ArmResult,
    /// The environment failed: OOM, upstream rate limit, adapter misconfiguration.
    Infrastructure,
    /// farmerbob terminated the run: cost duplicate, watchdog, divergence stop.
    OrchestratorCancelled,
    /// The task itself was incoherent, e.g. told to create a file that already existed.
    TaskInvalid,
    /// The harness, not the arm, produced the verdict.
    HarnessBug,
    /// The record is incomplete — no verdict was recorded at all. Distinct from a non-arm
    /// outcome: here we simply do not know what happened, and saying so is the point.
    Unknown,
}

impl OutcomeClass {
    /// Only an arm result may update an arm's accept/reject posterior.
    pub fn counts_for_posterior(self) -> bool {
        matches!(self, OutcomeClass::ArmResult)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            OutcomeClass::ArmResult => "arm_result",
            OutcomeClass::Infrastructure => "infrastructure",
            OutcomeClass::OrchestratorCancelled => "orchestrator_cancelled",
            OutcomeClass::TaskInvalid => "task_invalid",
            OutcomeClass::HarnessBug => "harness_bug",
            OutcomeClass::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RunRecord {
    pub source: String,
    pub bead: String,
    #[serde(default)]
    pub verdict: Option<String>,
    #[serde(default)]
    pub outcome_class: Option<String>,
    #[serde(default)]
    pub rc: Option<i32>,
    #[serde(default)]
    pub duration_s: Option<i64>,
    #[serde(default)]
    pub lines_added: Option<u64>,
    #[serde(default)]
    pub tests_run: Option<u64>,
    #[serde(default)]
    pub mem_peak_mb: Option<u64>,
    #[serde(default)]
    pub worktree: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
}

impl RunRecord {
    /// Classify the outcome, preferring an explicit label over inference.
    ///
    /// Exit status is a weak signal and is only consulted as a fallback: the *killer* knows
    /// why it killed something and the scorer does not, so a run terminated by farmerbob
    /// should be tagged at the moment of the kill.
    pub fn classify(&self) -> OutcomeClass {
        if let Some(explicit) = self.outcome_class.as_deref() {
            return match explicit {
                "infrastructure" => OutcomeClass::Infrastructure,
                "orchestrator_cancelled" => OutcomeClass::OrchestratorCancelled,
                "task_invalid" => OutcomeClass::TaskInvalid,
                "harness_bug" => OutcomeClass::HarnessBug,
                _ => OutcomeClass::ArmResult,
            };
        }
        if self.verdict.as_deref() == Some("TASK-INVALID") {
            return OutcomeClass::TaskInvalid;
        }
        match self.rc {
            // 128+SIGTERM and 128+SIGKILL: something stopped this run deliberately.
            Some(143) | Some(137) => OutcomeClass::OrchestratorCancelled,
            // No verdict recorded. The early harness did not write one, so the run is
            // unscoreable rather than failed -- "we don't know" must be representable.
            _ if self.verdict.is_none() => OutcomeClass::Unknown,
            _ => OutcomeClass::ArmResult,
        }
    }

    /// Did the arm succeed? `None` when the outcome says nothing about the arm.
    pub fn accepted(&self) -> Option<bool> {
        if !self.classify().counts_for_posterior() {
            return None;
        }
        self.verdict.as_deref().map(|v| v == "PASS")
    }
}

/// Load every run record in a directory, skipping the harness's aggregate report files.
pub fn load_dir(dir: &Path) -> Result<Vec<(PathBuf, RunRecord)>, String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.ends_with(".json") {
            continue;
        }
        // aggregates, not runs
        if name.contains(".score.")
            || name.contains(".compare.")
            || name.contains(".crossx.")
            || name.contains(".verify.")
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        match serde_json::from_str::<RunRecord>(&text) {
            Ok(r) => out.push((path, r)),
            // a malformed record is a harness fault; skip it rather than abort the import
            Err(_) => continue,
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(verdict: &str, rc: i32, class: Option<&str>) -> RunRecord {
        RunRecord {
            source: "arm".into(),
            bead: "task".into(),
            verdict: Some(verdict.into()),
            outcome_class: class.map(String::from),
            rc: Some(rc),
            duration_s: Some(1),
            lines_added: Some(1),
            tests_run: Some(1),
            mem_peak_mb: None,
            worktree: None,
            branch: None,
        }
    }

    #[test]
    fn a_pass_is_an_arm_result_and_counts() {
        let r = rec("PASS", 0, None);
        assert_eq!(r.classify(), OutcomeClass::ArmResult);
        assert_eq!(r.accepted(), Some(true));
    }

    #[test]
    fn a_genuine_failure_counts_against_the_arm() {
        assert_eq!(rec("NO-COMPILE", 0, None).accepted(), Some(false));
    }

    #[test]
    fn a_sigterm_is_a_cancellation_not_a_failure() {
        // 143 = 128+15. farmerbob killed a paid-duplicate run mid-flight; the arm had been
        // working productively for 404 seconds.
        let r = rec("NO-OP", 143, None);
        assert_eq!(r.classify(), OutcomeClass::OrchestratorCancelled);
        assert_eq!(r.accepted(), None, "our own kill must not label the arm");
    }

    #[test]
    fn an_explicit_class_beats_the_exit_code() {
        let r = rec("NO-OP", 0, Some("infrastructure"));
        assert_eq!(r.classify(), OutcomeClass::Infrastructure);
        assert_eq!(r.accepted(), None);
    }

    #[test]
    fn an_invalid_task_never_scores_the_arm() {
        let r = rec("TASK-INVALID", -1, None);
        assert_eq!(r.classify(), OutcomeClass::TaskInvalid);
        assert_eq!(r.accepted(), None);
    }

    #[test]
    fn a_record_with_no_verdict_is_unknown_not_a_failure() {
        let r = RunRecord {
            verdict: None,
            ..rec("PASS", 0, None)
        };
        assert_eq!(r.classify(), OutcomeClass::Unknown);
        assert_eq!(
            r.accepted(),
            None,
            "an incomplete record is not evidence of anything"
        );
    }

    #[test]
    fn only_arm_results_reach_the_posterior() {
        for c in [
            OutcomeClass::Infrastructure,
            OutcomeClass::OrchestratorCancelled,
            OutcomeClass::TaskInvalid,
            OutcomeClass::HarnessBug,
            OutcomeClass::Unknown,
        ] {
            assert!(!c.counts_for_posterior(), "{} must be excluded", c.as_str());
        }
        assert!(OutcomeClass::ArmResult.counts_for_posterior());
    }
}
