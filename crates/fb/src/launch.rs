//! The launcher table: how to invoke each arm. Ported from fb-launch.sh.
//!
//! One table, in one language. It existed twice before -- here in shell and again in
//! `pareto.rs` as a private map -- and production called the wrong copy, so an entry added
//! for agy arms was unreachable and changed nothing. Three critics found that
//! independently (bead farmerbob-lzae).
//!
//! `argv_for` is pure: it takes the arm, its model and the working directory and returns
//! the command, so the table can be tested without spawning anything. `launch` runs it.

use farmerbob_core::measurement::Measurement;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The confinement wrapper every arm is run under.
fn isolated() -> PathBuf {
    crate::paths::repo().join("fb-isolated")
}

/// Why an arm cannot be launched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoLauncher {
    /// The arm name matches no entry in the table. NOT a guess at a default: a wrong
    /// launcher runs the wrong model and bills the wrong account.
    UnknownArm(String),
}

/// The argv for one arm, after the `fb-isolated <wd>` prefix.
///
/// `continue_session` asks the launcher to resume the arm's previous turn rather than start
/// a new one, which is how a follow-up is handed back to the author that wrote the code.
pub fn argv_for(
    arm: &str,
    model: &str,
    wd: &Path,
    prompt: &str,
    continue_session: bool,
) -> Result<Vec<String>, NoLauncher> {
    let wd = wd.to_string_lossy().to_string();
    let s = |v: &str| v.to_string();
    let mut argv: Vec<String> = Vec::new();
    match arm {
        "codex-luna" => {
            argv.push(s("codex"));
            argv.push(s("exec"));
            if continue_session {
                argv.push(s("resume"));
                argv.push(s("--last"));
            }
            argv.extend([
                s("--dangerously-bypass-approvals-and-sandbox"),
                s("-m"),
                s("gpt-5.6-luna"),
                s(prompt),
            ]);
        }
        "glm-53-flash" => {
            argv.push(s("zcode"));
            if continue_session {
                argv.push(s("--continue"));
            }
            argv.extend([s("--prompt"), s(prompt)]);
        }
        "claude-sonnet" => {
            argv.extend([
                s("claude"),
                s("-p"),
                s(prompt),
                s("--permission-mode"),
                s("bypassPermissions"),
                s("--model"),
                s("sonnet"),
                s("--effort"),
                std::env::var("FB_CLAUDE_EFFORT").unwrap_or_else(|_| s("xhigh")),
            ]);
            if continue_session {
                argv.push(s("--continue"));
            }
            argv.extend([s("--output-format"), s("json")]);
        }
        a if a == "gemini-38-flash" || a.starts_with("agy-") => {
            argv.extend([
                s("agy"),
                s("-p"),
                s(prompt),
                s("--print-timeout"),
                s("45m"),
                s("--model"),
                s(model),
                s("--add-dir"),
                wd.clone(),
            ]);
            if continue_session {
                argv.push(s("--continue"));
            }
            argv.extend([
                s("--dangerously-skip-permissions"),
                s("--output-format"),
                s("text"),
            ]);
        }
        a if a.starts_with("or-") => {
            argv.extend([
                s("ori"),
                s("opencode"),
                s("run"),
                s("--dir"),
                wd.clone(),
                s("--auto"),
            ]);
            if continue_session {
                argv.push(s("--continue"));
            }
            argv.extend([s("-m"), s(model), s(prompt)]);
        }
        a if a.starts_with("oc-") || a.starts_with("ifm-") => {
            argv.extend([s("opencode"), s("run"), s("--dir"), wd.clone(), s("--auto")]);
            if continue_session {
                argv.push(s("--continue"));
            }
            argv.extend([s("-m"), s(model), s(prompt)]);
        }
        other => return Err(NoLauncher::UnknownArm(other.to_string())),
    }
    Ok(argv)
}

/// The model an arm runs, from the registry. Empty when the registry does not say, which
/// the launchers that need it will reject for themselves.
pub fn model_for(arm: &str) -> String {
    crate::sources::Registry::load(&crate::paths::repo().join("sources.toml"))
        .ok()
        .and_then(|r| r.sources.get(arm).map(|s| s.model.clone()))
        .unwrap_or_default()
}

/// Run one arm in `wd`, capturing everything it says.
///
/// The output is RETURNED, not discarded. `fb critique` ran its launcher with
/// `Stdio::null()` on both streams and wrote a zero-byte log beside the prompt, so a critic
/// that crashed on startup and one that ran and chose to write nothing were byte-identical.
/// Three runs could not be diagnosed at all before that was fixed.
pub fn launch(
    arm: &str,
    prompt: &str,
    wd: &Path,
    continue_session: bool,
    env: &[(&str, PathBuf)],
) -> Measurement<String> {
    let model = model_for(arm);
    let argv = match argv_for(arm, &model, wd, prompt, continue_session) {
        Ok(argv) => argv,
        Err(NoLauncher::UnknownArm(a)) => {
            return Measurement::instrument_failed(&format!(
                "no launcher for arm {a}: sources.toml names it but the table does not"
            ));
        }
    };
    // ifm-* arms authenticate from a secrets file the launcher reads itself; every other
    // family authenticates from a credential the isolated environment must carry.
    let mut cmd = Command::new(isolated());
    cmd.arg(wd).args(&argv).current_dir(wd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    match cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).output() {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            let err = String::from_utf8_lossy(&out.stderr);
            if !err.is_empty() {
                text.push_str("\n=== stderr ===\n");
                text.push_str(&err);
            }
            if out.status.success() {
                Measurement::observed(text)
            } else {
                Measurement::instrument_failed(&format!(
                    "{arm} exited with {}: {}",
                    out.status,
                    text.chars().take(400).collect::<String>()
                ))
            }
        }
        Err(e) => Measurement::instrument_failed(&format!("cannot spawn {arm}: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(arm: &str, cont: bool) -> Vec<String> {
        argv_for(arm, "some/model", Path::new("/w"), "P", cont).expect("known arm")
    }

    /// An arm the table does not know is an ERROR, never a default. A wrong launcher runs
    /// the wrong model and bills the wrong account.
    #[test]
    fn an_unknown_arm_has_no_launcher() {
        assert_eq!(
            argv_for("nobody", "m", Path::new("/w"), "P", false),
            Err(NoLauncher::UnknownArm("nobody".into()))
        );
    }

    /// Family prefixes route to their launcher: or-* through ori, oc-* and ifm-* direct.
    /// This is the distinction fb-launch.sh drew and pareto.rs's private copy did not.
    #[test]
    fn families_route_by_prefix() {
        assert_eq!(argv("or-anything", false)[0], "ori");
        assert_eq!(argv("oc-anything", false)[0], "opencode");
        assert_eq!(argv("ifm-anything", false)[0], "opencode");
        assert_eq!(argv("agy-anything", false)[0], "agy");
        assert_eq!(argv("gemini-38-flash", false)[0], "agy");
    }

    /// Continuation is what hands a follow-up back to the arm that wrote the code. codex
    /// spells it `exec resume --last`; every other launcher spells it `--continue`.
    #[test]
    fn continuation_is_spelled_per_launcher() {
        let c = argv("codex-luna", true);
        assert!(c.windows(2).any(|w| w == ["exec", "resume"]), "{c:?}");
        assert!(c.contains(&"--last".to_string()));
        assert!(argv("or-x", true).contains(&"--continue".to_string()));
        assert!(argv("glm-53-flash", true).contains(&"--continue".to_string()));
    }

    /// The same arm without continuation must NOT carry it. Pinned against the test above:
    /// one flag different, and resuming when a fresh turn was wanted makes an arm re-answer
    /// its previous prompt.
    #[test]
    fn a_fresh_turn_carries_no_continuation() {
        let f = argv("codex-luna", false);
        assert!(!f.contains(&"resume".to_string()), "{f:?}");
        assert!(!argv("or-x", false).contains(&"--continue".to_string()));
    }

    /// Every launcher is handed the prompt and, where it needs one, the working directory.
    #[test]
    fn the_prompt_reaches_every_launcher() {
        for arm in [
            "codex-luna",
            "glm-53-flash",
            "or-x",
            "oc-x",
            "agy-x",
            "claude-sonnet",
        ] {
            assert!(
                argv(arm, false).contains(&"P".to_string()),
                "{arm} did not receive the prompt"
            );
        }
    }
}
