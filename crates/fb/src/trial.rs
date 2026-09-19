//! `fb trial` — drive a whole bakeoff from one command.
//!
//! A trial is: dispatch N implementers into isolated worktrees, score them objectively,
//! cross-review, promote the critics' claims into executed tests, and print the evidence the
//! adjudicator needs. Every stage is resumable, because stages fail independently and
//! re-running an expensive dispatch to redo a cheap score is waste.
//!
//! The stages shell out to the harness scripts. Those have been debugged against ~70 real
//! runs and know things that are easy to get wrong — per-run state isolation, the per-vendor
//! concurrency cap, the reference veto, that `error: test failed` is not a compile error.
//! Rewriting them in Rust for tidiness would re-earn those bugs.

use std::path::PathBuf;
use std::process::Command;

pub const REPO: &str = "/home/gabe/Documents/farmerbob";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Dispatch,
    Score,
    Differential,
    Crossx,
    Critique,
    Promote,
    Prove,
    Report,
}

impl Stage {
    pub fn all() -> &'static [Stage] {
        &[
            Stage::Dispatch,
            Stage::Score,
            Stage::Differential,
            Stage::Crossx,
            Stage::Critique,
            Stage::Promote,
            Stage::Prove,
            Stage::Report,
        ]
    }

    pub fn name(self) -> &'static str {
        match self {
            Stage::Dispatch => "dispatch",
            Stage::Score => "score",
            Stage::Differential => "differential",
            Stage::Crossx => "crossx",
            Stage::Critique => "critique",
            Stage::Promote => "promote",
            Stage::Prove => "prove",
            Stage::Report => "report",
        }
    }

    pub fn parse(s: &str) -> Option<Stage> {
        Stage::all().iter().copied().find(|st| st.name() == s)
    }

    /// Stages from `from` onwards, so a trial can resume rather than restart.
    pub fn from_here(from: Stage) -> Vec<Stage> {
        let all = Stage::all();
        let i = all.iter().position(|s| *s == from).unwrap_or(0);
        all[i..].to_vec()
    }
}

pub struct Trial {
    pub task: String,
    pub krate: String,
    pub target: String,
    pub arms: Vec<String>,
    pub prover: String,
}

impl Trial {
    fn logs() -> PathBuf {
        crate::paths::logs()
    }

    fn sh(&self, script: &str, args: &[&str]) -> bool {
        eprintln!("  $ {script} {}", args.join(" "));
        Command::new(format!("{REPO}/{script}"))
            .args(args)
            .current_dir(REPO)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// `fb differential` answers with FOUR exit codes and two of them look alike:
    /// 0 every measured arm agrees, 1 an arm diverges -- both of which mean the
    /// INSTRUMENT WORKED -- 3 the task declares no oracle, and 2 declared but
    /// unrunnable. Only 2 is a stage failure, because it is the only one where
    /// something should have been checked and was not. `sh` collapses all of them
    /// to `status.success()`, which would fail a trial for finding a divergence:
    /// the stage's whole purpose.
    fn differential(&self) -> bool {
        eprintln!("  $ fb differential {}", self.task);
        let code = Command::new(format!("{REPO}/target/debug/fb"))
            .args(["differential", &self.task])
            .current_dir(REPO)
            .status()
            .ok()
            .and_then(|s| s.code());
        match code {
            Some(0) => { eprintln!("  all measured arms agree"); true }
            Some(1) => { eprintln!("  DIVERGENCE FOUND -- a finding, and a successful measurement"); true }
            Some(3) => { eprintln!("  n/a, this task declares no oracle"); true }
            Some(2) => { eprintln!("  DECLARED BUT COULD NOT RUN -- a gap, not a pass"); false }
            other => { eprintln!("  unexpected exit {other:?}"); false }
        }
    }

    /// `fb crossx` answers with FOUR codes and, like differential, two of them
    /// are not failures: `4` means FEWER THAN TWO CANDIDATES, which under
    /// one-arm-per-task is the expected state and not a fault in anything. It
    /// used to return `1` for that and for a usage error alike, so a trial of a
    /// single-arm task halted here and four of its eight stages were
    /// unreachable.  (bead farmerbob-9ef2)
    fn crossx(&self) -> bool {
        eprintln!("  $ fb crossx {} {} {}", self.task, self.krate, self.target);
        let code = Command::new(format!("{REPO}/target/debug/fb"))
            .args(["crossx", &self.task, "--crate", &self.krate, &self.target])
            .current_dir(REPO)
            .status()
            .ok()
            .and_then(|s| s.code());
        match code {
            Some(0) => { eprintln!("  matrix complete"); true }
            Some(4) => { eprintln!("  n/a, fewer than two candidates -- no matrix to build"); true }
            Some(3) => { eprintln!("  VOID: the diagonal invariant failed; scores deliberately not written"); false }
            Some(1) => { eprintln!("  usage error"); false }
            other => { eprintln!("  unexpected exit {other:?}"); false }
        }
    }

    fn dispatch(&self) -> bool {
        // one matrix line; fb-admit applies memory slots and the per-vendor concurrency cap
        let matrix = format!("{REPO}/.fb/trial-{}.tsv", self.task);
        let line = format!("{}\t{}\t{}\n", self.task, self.krate, self.arms.join(","));
        if std::fs::write(&matrix, line).is_err() {
            eprintln!("  cannot write {matrix}");
            return false;
        }
        self.sh("fb-admit.sh", &[&matrix])
    }

    fn report(&self) -> bool {
        // the adjudication table: objective evidence, then what the critics said
        let _ = self.sh("fb-objective.sh", &[&self.task]);
        let cdir = Self::logs().join("critiques").join(&self.task);
        println!("\n=== subjective: critiques ===");
        if let Ok(entries) = std::fs::read_dir(&cdir) {
            let mut any = false;
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if !name.contains(".on.") || !name.ends_with(".md") {
                    continue;
                }
                any = true;
                let body = std::fs::read_to_string(e.path()).unwrap_or_default();
                let claims = body.matches("CLAIM:").count();
                let clean = body.contains("NO MATERIAL DEFECTS");
                let stem = name.trim_end_matches(".md");
                let (critic, subject) = stem.split_once(".on.").unwrap_or((stem, "?"));
                println!(
                    "  {critic:<22} -> {subject:<22} {claims} claims{}",
                    if clean { "  (no material defects)" } else { "" }
                );
            }
            if !any {
                println!("  (none)");
            }
        } else {
            println!("  (none)");
        }
        true
    }

    /// Where the trial's evidence is persisted. Printing it is not enough: a pipe that
    /// filters stdout can swallow the whole report, and then an empty output reads as
    /// "nothing happened" rather than "you filtered it". The artefact on disk is the
    /// reliable source.
    pub fn report_path(&self) -> PathBuf {
        Self::logs().join(format!("{}.trial.txt", self.task))
    }

    pub fn run(&self, from: Stage) -> i32 {
        for stage in Stage::from_here(from) {
            eprintln!("\n── {} ──────────────────────────────", stage.name());
            let ok = match stage {
                Stage::Dispatch => self.dispatch(),
                Stage::Score => self.sh("fb-score.sh", &[&self.task, &self.krate]),
                Stage::Differential => self.differential(),
                Stage::Crossx => self.crossx(),
                Stage::Critique => {
                    self.sh("fb-critique.sh", &[&self.task, &self.krate, &self.target])
                }
                Stage::Promote => self.sh("fb-promote.sh", &[&self.task]),
                Stage::Prove => self.sh(
                    "fb-prove.sh",
                    &[&self.task, &self.krate, &self.target, &self.prover],
                ),
                Stage::Report => self.report(),
            };
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.report_path())
                .and_then(|mut f| {
                    use std::io::Write;
                    writeln!(f, "{}\t{}", stage.name(), if ok { "ok" } else { "FAILED" })
                });
            if !ok {
                // A failed stage is reported and the trial stops there rather than pressing on
                // with missing evidence -- an adjudication table built on a stage that did not
                // run is worse than no table.
                eprintln!(
                    "  stage `{}` failed; resume with --from {}",
                    stage.name(),
                    stage.name()
                );
                return 1;
            }
        }
        eprintln!("\n  evidence: {}", self.report_path().display());
        0
    }
}
