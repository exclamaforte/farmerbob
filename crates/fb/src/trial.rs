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
    Critique,
    Promote,
    Prove,
    Report,
}

impl Stage {
    pub fn all() -> &'static [Stage] {
        &[Stage::Dispatch, Stage::Score, Stage::Critique, Stage::Promote, Stage::Prove, Stage::Report]
    }

    pub fn name(self) -> &'static str {
        match self {
            Stage::Dispatch => "dispatch",
            Stage::Score => "score",
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
                Stage::Critique => self.sh("fb-critique.sh", &[&self.task, &self.krate, &self.target]),
                Stage::Promote => self.sh("fb-promote.sh", &[&self.task]),
                Stage::Prove => {
                    self.sh("fb-prove.sh", &[&self.task, &self.krate, &self.target, &self.prover])
                }
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
                eprintln!("  stage `{}` failed; resume with --from {}", stage.name(), stage.name());
                return 1;
            }
        }
        eprintln!("\n  evidence: {}", self.report_path().display());
        0
    }
}
