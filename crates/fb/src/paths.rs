//! Where the harness keeps things, and how to point it somewhere else.
//!
//! Every ported command derived its own paths from `$HOME/.local/share/farmerbob`, hardcoded
//! at each call site. That made a differential check impossible to run safely: the only way
//! to compare a port against the shell it replaces was to let it write the real artefact
//! first. `fb objective` did exactly that -- it rebuilt `logs/objective.json`, the record
//! `fb pareto`, `fb select` and the bandit's posteriors all read, producing 220 rows where
//! the shell produces 137 -- before anything had established it was correct. The file
//! survived only because a copy had been taken by hand beforehand.
//!
//! A gate meant to prove a port safe before it touches anything should not prove it by
//! letting it touch everything. These overrides make a sandboxed run one environment
//! variable.
//!
//! A critic asked for this first, reviewing port-critique: "the missing FB_REPO/FB_WT/FB_LOGS
//! overrides plus hardcoded paths make review units untestable against fake trees."
//!   (bead farmerbob-jd2.14)

use std::path::{Path, PathBuf};

/// Resolve an override against a default. Pure, so it can be tested without mutating the
/// process environment -- the first version of this module tested the wrappers by setting
/// FB_LOGS, and since cargo runs tests in parallel and the environment is process-global,
/// one test redirected another's reads and the suite failed on its second run. A test that
/// has to mutate global state to say anything is testing the harness, not the logic.
fn pick(override_value: Option<&str>, fallback: PathBuf) -> PathBuf {
    match override_value {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => fallback,
    }
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// The farmerbob state directory: `$FB_STATE`, else `$HOME/.local/share/farmerbob`.
pub fn state() -> PathBuf {
    pick(env("FB_STATE").as_deref(), default_state())
}

fn default_state() -> PathBuf {
    match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h).join(".local/share/farmerbob"),
        Err(_) => PathBuf::from(".farmerbob"),
    }
}

/// Where artefacts are written: `$FB_LOGS`, else `<state>/logs`.
pub fn logs() -> PathBuf {
    pick(env("FB_LOGS").as_deref(), state().join("logs"))
}

/// Candidate worktrees: `$FB_WT`, else `<state>/worktrees`.
pub fn worktrees() -> PathBuf {
    pick(env("FB_WT").as_deref(), state().join("worktrees"))
}

/// The repository root: `$FB_REPO`, else the working directory.
pub fn repo() -> PathBuf {
    pick(
        env("FB_REPO").as_deref(),
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point: a differential can be redirected away from the real artefacts.
    #[test]
    fn an_override_redirects_and_an_absent_one_does_not() {
        let fallback = PathBuf::from("/real/logs");
        assert_eq!(pick(Some("/tmp/sandbox"), fallback.clone()), PathBuf::from("/tmp/sandbox"));
        assert_eq!(pick(None, fallback.clone()), fallback);
    }

    /// An empty variable is not an override. `FB_LOGS=` in a script is a typo, not a request
    /// to write artefacts into the current directory.
    #[test]
    fn an_empty_override_is_ignored() {
        let fallback = PathBuf::from("/real/logs");
        assert_eq!(pick(Some(""), fallback.clone()), fallback);
    }

    /// Defaults nest correctly, checked without touching the environment.
    #[test]
    fn logs_and_worktrees_sit_under_the_state_dir() {
        let st = PathBuf::from("/s");
        assert_eq!(pick(None, st.join("logs")), Path::new("/s/logs"));
        assert_eq!(pick(None, st.join("worktrees")), Path::new("/s/worktrees"));
    }
}
