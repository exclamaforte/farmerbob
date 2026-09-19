//! `fb dispatch <arm> <task> <prompt> [crate]` — run one arm on one task, confined.
//!
//! Ported from fb-dispatch.sh. The decisions are pure and tested here; the process
//! orchestration -- worktree, systemd scope, launch -- is not, because it cannot be.

use farmerbob_core::target_decl::{self, Declaration};
use std::fs;
use std::path::{Path, PathBuf};

/// Paths the harness strips from every worktree before an arm starts.
///
/// The arm must not be handed the orchestrator's issue tracker, the other tasks' prompts,
/// or the hidden conformance suites -- `.fb/` is TRACKED, so every worktree shipped the
/// tests an arm was about to be measured against.
pub const STRIPPED: [&str; 10] = [
    "CLAUDE.md",
    "AGENTS.md",
    ".beads",
    ".cursor",
    ".codex",
    ".agents",
    ".fb/conformance",
    ".fb/defects",
    ".fb/prompts",
    ".fb/known-defects",
];

/// Whether the task's precondition holds on the base it will run against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Precondition {
    /// The task can run.
    Holds,
    /// It cannot, and this is why. A `creates` task whose target exists would correctly
    /// no-op; a `modifies` task whose target is missing has nothing to modify. Either way
    /// the arm is not at fault and the run must not be scored as its failure.
    Invalid(String),
    /// The spec declares nothing. Other stages report that; it is not dispatch's refusal.
    NoDeclaration,
}

/// Check every declared deliverable against the base.
pub fn precondition(spec: &str, exists: &impl Fn(&str) -> bool) -> Precondition {
    let Ok(declarations) = target_decl::declared_all(spec) else {
        return Precondition::NoDeclaration;
    };
    for d in &declarations {
        let path = target_decl::path(d);
        match d {
            Declaration::Creates(_) if exists(path) => {
                return Precondition::Invalid(format!(
                    "declares creates:{path} but it already exists on the base"
                ));
            }
            Declaration::Modifies(_) if !exists(path) => {
                return Precondition::Invalid(format!(
                    "declares modifies:{path} but it does not exist on the base"
                ));
            }
            _ => {}
        }
    }
    Precondition::Holds
}

/// Why an `fb:reads` path is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadRefusal {
    /// Absolute: it names something outside the worktree by construction.
    Absolute,
    /// Contains `..`: it escapes the repository.
    Escapes,
    /// A dotfile or dot-directory: that is where the harness keeps what the arm must not
    /// see, and `fb:reads .fb/conformance` would hand an arm its own hidden suite.
    Dotted,
}

/// The extra files a spec asks to be restored, validated.
///
/// A task ABOUT a harness script needs that script present, which the strip above removes.
/// Every path is checked, because this is the one place a spec can name a file to copy in.
pub fn reads_of(spec: &str) -> Vec<Result<String, (String, ReadRefusal)>> {
    let mut out = Vec::new();
    for line in spec.lines() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix("<!-- fb:reads ") else {
            continue;
        };
        let Some(body) = rest.strip_suffix("-->") else {
            continue;
        };
        for path in body.split_whitespace() {
            out.push(validate_read(path));
        }
    }
    out
}

fn validate_read(path: &str) -> Result<String, (String, ReadRefusal)> {
    if path.starts_with('/') {
        return Err((path.to_string(), ReadRefusal::Absolute));
    }
    if path == ".." || path.split('/').any(|c| c == "..") {
        return Err((path.to_string(), ReadRefusal::Escapes));
    }
    if path.starts_with('.') {
        return Err((path.to_string(), ReadRefusal::Dotted));
    }
    Ok(path.to_string())
}

/// The prompt an arm is given when it already wrote this task's spec critique.
///
/// Without this, codex resumed its critique session and re-answered the spec instead of
/// implementing it (bead farmerbob-81tg).
pub fn continuation_prompt(task: &str) -> String {
    format!(
        "Your spec critique of this task is COMPLETE and has been recorded. This turn is the\n\
         IMPLEMENTATION turn: write the code the task below specifies. Do not critique the spec\n\
         again and do not write .fb/speccheck.md -- that file belongs to the previous turn and\n\
         anything you write to it now is discarded.\n\
         Everything you found while critiquing still applies. Where the spec is defective, the\n\
         task below says what to do: implement the closest honest thing and say so in your\n\
         handoff.\n\
         {task}"
    )
}

/// The run's identifiers.
pub fn run_id(task: &str, arm: &str) -> String {
    format!("{task}--{arm}")
}

/// The systemd unit name for a run: a scope name may carry only a restricted alphabet.
pub fn unit_name(run: &str, pid: u32) -> String {
    let safe: String = run
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("fb-{safe}-{pid}")
}

/// Remove everything the harness owns from a worktree.
pub fn strip(wt: &Path) {
    for entry in STRIPPED {
        let p = wt.join(entry);
        if p.is_dir() {
            let _ = fs::remove_dir_all(&p);
        } else {
            let _ = fs::remove_file(&p);
        }
    }
    // The harness's own scripts, and any queue file that rode along.
    if let Ok(entries) = fs::read_dir(wt) {
        for e in entries.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with("fb-") && n.ends_with(".sh") {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    if let Ok(entries) = fs::read_dir(wt.join(".fb")) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().ends_with(".tsv") {
                let _ = fs::remove_file(e.path());
            }
        }
    }
}

/// Seed the credentials an isolated XDG environment would otherwise hide.
///
/// opencode keeps provider keys in `$XDG_DATA_HOME/opencode/auth.json`, and the per-run
/// isolation handed every run a fresh empty directory -- so three arms across two vendors
/// died in under eight seconds each with a bare "Unexpected server error" that named
/// neither auth nor us (bead farmerbob-kpch).
pub fn seed_credentials(state_data: &Path) -> Result<(), String> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Err("HOME is unset".to_string());
    };
    let cred = home.join(".local/share/opencode/auth.json");
    if !cred.is_file() {
        return Ok(());
    }
    let dest = state_data.join("opencode");
    fs::create_dir_all(&dest).map_err(|e| format!("cannot create {}: {e}", dest.display()))?;
    fs::copy(&cred, dest.join("auth.json"))
        .map(|_| ())
        .map_err(|e| format!("cannot seed {}: {e}", cred.display()))
}

/// Dispatch one arm on one task.
pub fn run(arm: &str, task: &str, prompt_file: &Path, krate: &str) -> i32 {
    let repo = crate::paths::repo();
    let wt_root = crate::paths::worktrees();
    let logs = crate::paths::logs();
    let run = run_id(task, arm);
    let wt = wt_root.join(&run);
    let branch = format!("fb/{task}/{arm}");
    let base = std::env::var("FB_BASE").unwrap_or_else(|_| "HEAD".to_string());
    let _ = fs::create_dir_all(&wt_root);
    let _ = fs::create_dir_all(&logs);

    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(&repo)
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    let wt_s = wt.to_string_lossy().to_string();
    let _ = git(&["worktree", "remove", "--force", &wt_s]);
    let _ = git(&["worktree", "prune"]);
    let _ = git(&["branch", "-D", &branch]);
    if !git(&["worktree", "add", "-q", "-b", &branch, &wt_s, &base]) {
        println!("{arm}: worktree failed");
        return 1;
    }

    let Ok(spec) = fs::read_to_string(prompt_file) else {
        println!("{arm}: cannot read {}", prompt_file.display());
        return 1;
    };
    // The precondition is checked against the WORKTREE, which is the base the arm will
    // actually see.
    let exists = |p: &str| wt.join(p).exists();
    if let Precondition::Invalid(why) = precondition(&spec, &exists) {
        println!("{arm:<24} {:<13} {why}", "TASK-INVALID");
        return 0;
    }

    let _ = fs::write(wt.join(".fb-task.md"), &spec);
    strip(&wt);

    for read in reads_of(&spec) {
        match read {
            Ok(path) => {
                let src = repo.join(&path);
                if !src.is_file() {
                    println!("{arm}: fb:reads names {path}, which does not exist");
                    return 1;
                }
                if let Some(parent) = wt.join(&path).parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let _ = fs::copy(&src, wt.join(&path));
                println!("{arm}: restored {path} for a task that is about it");
            }
            Err((path, why)) => {
                println!("{arm}: refusing fb:reads {path} ({why:?})");
                return 1;
            }
        }
    }

    let state = crate::paths::state().join("state").join(&run);
    let _ = fs::remove_dir_all(&state);
    for sub in ["data", "state", "cache"] {
        let _ = fs::create_dir_all(state.join(sub));
    }
    if let Err(e) = seed_credentials(&state.join("data")) {
        eprintln!("dispatch: {e}");
        return 1;
    }

    // An arm that already wrote this task's spec critique is CONTINUED, and told the
    // critique turn is over.
    let critiqued = logs
        .join("speccheck")
        .join(task)
        .join(format!("{arm}.findings.md"))
        .is_file();
    let prompt = if critiqued {
        println!("{arm}: continuing its spec-critique session");
        continuation_prompt(&spec)
    } else {
        spec.clone()
    };

    let env = [
        ("XDG_DATA_HOME", state.join("data")),
        ("XDG_STATE_HOME", state.join("state")),
        ("XDG_CACHE_HOME", state.join("cache")),
    ];
    let started = std::time::Instant::now();
    let unit = unit_name(&run, std::process::id());
    let said = crate::launch::launch_in(
        arm,
        &prompt,
        &wt,
        critiqued,
        &env,
        Some(crate::launch::Scope {
            unit: &unit,
            memory_max: std::env::var("FB_MEM_MAX").unwrap_or_else(|_| "3G".to_string()),
            runtime_max_s: std::env::var("FB_RUN_TIMEOUT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(2700),
        }),
    );
    let seconds = started.elapsed().as_secs();
    let log = logs.join(format!("{run}.log"));
    let (rc, text) = match &said {
        farmerbob_core::measurement::Measurement::Observed(t) => (0, t.clone()),
        farmerbob_core::measurement::Measurement::Missing(reason) => (1, format!("{reason:?}")),
    };
    let _ = fs::write(&log, &text);
    let _ = fs::write(
        logs.join(format!("{run}.json")),
        format!(
            r#"{{"source":"{arm}","bead":"{task}","crate":"{krate}","rc":{rc},"duration_s":{seconds},"worktree":"{}","branch":"{branch}","confined":"yes"}}"#,
            wt.display()
        ),
    );
    println!("{arm:<24} rc={rc} {seconds}s -> {}", log.display());
    rc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `creates` task whose target already exists cannot do anything but no-op, and the
    /// arm is not at fault. Pinned against the same declaration on a base where it is
    /// absent: one bit different, opposite answers.
    #[test]
    fn a_creates_task_is_invalid_when_the_target_exists() {
        let spec = "<!-- fb:creates crates/a/b.rs -->\n";
        assert!(matches!(
            precondition(spec, &|_| true),
            Precondition::Invalid(_)
        ));
        assert_eq!(precondition(spec, &|_| false), Precondition::Holds);
    }

    /// And a `modifies` task inverts it. The shell checked only `creates`, so a
    /// modifies-task against a deleted file dispatched and every arm correctly no-opped.
    #[test]
    fn a_modifies_task_is_invalid_when_the_target_is_missing() {
        let spec = "<!-- fb:modifies crates/a/b.rs -->\n";
        assert!(matches!(
            precondition(spec, &|_| false),
            Precondition::Invalid(_)
        ));
        assert_eq!(precondition(spec, &|_| true), Precondition::Holds);
    }

    /// EVERY declared file is checked, not just the first. A grouped task whose second
    /// deliverable is missing is as unrunnable as one whose first is.
    #[test]
    fn every_declared_file_is_checked() {
        let spec = "<!-- fb:modifies a.rs -->\n<!-- fb:modifies b.rs -->\n";
        let only_a = |p: &str| p == "a.rs";
        match precondition(spec, &only_a) {
            Precondition::Invalid(why) => assert!(why.contains("b.rs"), "{why}"),
            other => panic!("expected Invalid about b.rs, got {other:?}"),
        }
    }

    /// No declaration is not dispatch's refusal -- other stages report it.
    #[test]
    fn no_declaration_is_not_this_stages_refusal() {
        assert_eq!(
            precondition("# nothing\n", &|_| false),
            Precondition::NoDeclaration
        );
    }

    /// `fb:reads` is the one place a spec names a file to copy INTO the worktree, so every
    /// escape is refused: absolute paths, `..`, and dotfiles -- the last because `.fb/` is
    /// where the hidden conformance suites live, and `fb:reads .fb/conformance` would hand
    /// an arm the tests it is about to be measured against.
    #[test]
    fn fb_reads_refuses_every_escape() {
        let spec = "<!-- fb:reads /etc/passwd -->\n<!-- fb:reads ../x -->\n\
                    <!-- fb:reads .fb/conformance -->\n<!-- fb:reads fb-isolated -->\n";
        let got = reads_of(spec);
        assert_eq!(got.len(), 4);
        assert_eq!(got[0], Err(("/etc/passwd".into(), ReadRefusal::Absolute)));
        assert_eq!(got[1], Err(("../x".into(), ReadRefusal::Escapes)));
        assert_eq!(got[2], Err((".fb/conformance".into(), ReadRefusal::Dotted)));
        assert_eq!(got[3], Ok("fb-isolated".to_string()));
    }

    /// A `..` in the MIDDLE escapes too, not only at the start.
    #[test]
    fn a_dotdot_anywhere_escapes() {
        assert_eq!(
            validate_read("crates/../../etc/x"),
            Err(("crates/../../etc/x".into(), ReadRefusal::Escapes))
        );
    }

    /// A unit name carries only what systemd accepts, and includes the pid so two runs of
    /// one task cannot collide on a scope.
    #[test]
    fn a_unit_name_is_sanitised_and_unique() {
        let u = unit_name("my.task--arm/x", 42);
        assert!(!u.contains('.') && !u.contains('/'), "{u}");
        assert!(u.ends_with("-42"), "{u}");
        assert_ne!(unit_name("t--a", 1), unit_name("t--a", 2));
    }

    /// The continuation prompt tells the arm its critique turn is over. Without it codex
    /// resumed the critique session and re-answered the spec instead of implementing
    /// (bead farmerbob-81tg).
    #[test]
    fn the_continuation_prompt_ends_the_critique_turn() {
        let p = continuation_prompt("TASK BODY");
        assert!(p.contains("IMPLEMENTATION turn"), "{p}");
        assert!(p.contains("Do not critique the spec"), "{p}");
        assert!(p.ends_with("TASK BODY"), "the task is last");
    }
}
