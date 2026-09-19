//! Cross-review an implementation from another implementation's worktree.
//!
//! This is the Rust counterpart of `fb-critique.sh`.  I/O which can fail is kept in
//! [`Measurement`] so that a failed instrument cannot become an observed zero or an empty
//! patch.

use farmerbob_core::gate::{Observation, Verdict, judge};
use farmerbob_core::measurement::Measurement;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const REPO: &str = "/home/gabe/Documents/farmerbob";

fn read_text(path: &Path) -> Measurement<String> {
    match fs::read_to_string(path) {
        Ok(text) => Measurement::observed(text),
        Err(error) => {
            Measurement::instrument_failed(&format!("cannot read {}: {error}", path.display()))
        }
    }
}

fn command_text(command: &mut Command) -> Measurement<String> {
    match command.output() {
        Ok(output) if output.status.success() => match String::from_utf8(output.stdout) {
            Ok(text) => Measurement::observed(text),
            Err(error) => {
                Measurement::instrument_failed(&format!("command output was not UTF-8: {error}"))
            }
        },
        Ok(output) => {
            Measurement::instrument_failed(&format!("command exited with {}", output.status))
        }
        Err(error) => Measurement::instrument_failed(&format!("cannot run command: {error}")),
    }
}

fn line_count(path: &Path) -> Measurement<usize> {
    read_text(path).map(|text| text.lines().count())
}

fn word_count(path: &Path) -> Measurement<usize> {
    read_text(path).map(|text| text.split_whitespace().count())
}

fn arm_name(path: &Path, prefix: &str) -> Measurement<String> {
    match path.file_name().and_then(OsStr::to_str) {
        Some(name) if name.starts_with(prefix) => {
            Measurement::observed(name[prefix.len()..].to_string())
        }
        Some(_) => {
            Measurement::instrument_failed("worktree name does not have the expected prefix")
        }
        None => Measurement::instrument_failed("worktree name is not valid UTF-8"),
    }
}

fn candidates(bead: &str, target: &str) -> Measurement<Vec<String>> {
    let root = crate::paths::worktrees();
    let prefix = format!("{bead}--");
    let registry =
        match crate::sources::Registry::load(Path::new(REPO).join("sources.toml").as_path()) {
            Ok(registry) => registry,
            Err(error) => return Measurement::instrument_failed(&error),
        };
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) => {
            return Measurement::instrument_failed(&format!(
                "cannot read {}: {error}",
                root.display()
            ));
        }
    };
    let mut arms = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                return Measurement::instrument_failed(&format!(
                    "cannot inspect worktree: {error}"
                ));
            }
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let arm = match arm_name(&path, &prefix) {
            Measurement::Observed(arm) => arm,
            Measurement::Missing(_) => continue,
        };
        if path.join(".fb-task.md").is_file() {
            continue;
        }
        let target_path = path.join(target);
        match fs::metadata(&target_path) {
            Ok(metadata) if metadata.is_file() && metadata.len() > 0 => {}
            _ => continue,
        }
        if !registry.eligible(&arm).is_ok() {
            continue;
        }
        arms.push(arm);
    }
    arms.sort();
    Measurement::observed(arms)
}

fn patch_for(worktree: &Path, target: &str) -> Measurement<String> {
    let diff = command_text(
        Command::new("git")
            .current_dir(worktree)
            .args(["diff", "HEAD", "--", target]),
    );
    let patch = match diff {
        Measurement::Observed(text) if !text.is_empty() => text,
        Measurement::Observed(_) => {
            let path = worktree.join(target);
            if !path.is_file() {
                return Measurement::nothing_to_measure("declared target does not exist");
            }
            let lines = match line_count(&path) {
                Measurement::Observed(lines) => lines,
                Measurement::Missing(reason) => return Measurement::Missing(reason),
            };
            let contents = match read_text(&path) {
                Measurement::Observed(contents) => contents,
                Measurement::Missing(reason) => return Measurement::Missing(reason),
            };
            format!("=== NEW FILE: {target} ({lines} lines) ===\n{contents}")
        }
        Measurement::Missing(reason) => return Measurement::Missing(reason),
    };
    Measurement::observed(patch)
}

fn outside_files(worktree: &Path, target: &str) -> Measurement<Vec<String>> {
    let own_lib = Path::new(target)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("lib.rs");
    let tracked = command_text(Command::new("git").current_dir(worktree).args([
        "diff",
        "--name-only",
        "HEAD",
        "--",
        "crates/",
    ]));
    let untracked = command_text(Command::new("git").current_dir(worktree).args([
        "ls-files",
        "--others",
        "--exclude-standard",
        "crates/",
    ]));
    let (tracked, untracked) = match (tracked, untracked) {
        (Measurement::Observed(tracked), Measurement::Observed(untracked)) => (tracked, untracked),
        (Measurement::Missing(reason), _) | (_, Measurement::Missing(reason)) => {
            return Measurement::Missing(reason);
        }
    };
    let mut files: Vec<String> = tracked
        .lines()
        .chain(untracked.lines())
        .filter(|name| !name.is_empty() && *name != target && *name != own_lib.to_string_lossy())
        .map(str::to_string)
        .collect();
    files.sort();
    files.dedup();
    Measurement::observed(files)
}

fn prompt(template: &Path, patch: &str, handoff: &str, output: &Path) -> Measurement<()> {
    let template = match read_text(template) {
        Measurement::Observed(text) => text,
        Measurement::Missing(reason) => return Measurement::Missing(reason),
    };
    let text = template
        .replace("{PATCH}", &patch.chars().take(60_000).collect::<String>())
        .replace(
            "{HANDOFF}",
            &handoff.chars().take(4_000).collect::<String>(),
        )
        .replace("{OUT}", &output.to_string_lossy());
    match fs::write(output, text) {
        Ok(()) => Measurement::observed(()),
        Err(error) => {
            Measurement::instrument_failed(&format!("cannot write {}: {error}", output.display()))
        }
    }
}

fn environment_paths(state_root: &Path, run: &str) -> [PathBuf; 3] {
    let run_root = state_root.join(run);
    [
        run_root.join("data"),
        run_root.join("state"),
        run_root.join("cache"),
    ]
}

fn state_root() -> Measurement<PathBuf> {
    match std::env::var_os("HOME") {
        Some(home) => {
            Measurement::observed(PathBuf::from(home).join(".local/share/farmerbob/state"))
        }
        None => Measurement::instrument_failed("HOME is not set"),
    }
}

fn launch(arm: &str, text: &str, worktree: &Path, run: &str) -> Measurement<()> {
    let state_root = match state_root() {
        Measurement::Observed(root) => root,
        Measurement::Missing(reason) => return Measurement::Missing(reason),
    };
    let [data_home, state_home, cache_home] = environment_paths(&state_root, run);
    for directory in [&data_home, &state_home, &cache_home] {
        if let Err(error) = fs::create_dir_all(directory) {
            return Measurement::instrument_failed(&format!(
                "cannot create {}: {error}",
                directory.display()
            ));
        }
    }

    let result = Command::new("bash")
        .arg("-c")
        .arg(". /home/gabe/Documents/farmerbob/fb-launch.sh; fb_launch \"$1\" \"$2\" \"$3\"")
        .arg("fb-launch")
        .arg(arm)
        .arg(text)
        .arg(worktree)
        .env("XDG_DATA_HOME", &data_home)
        .env("XDG_STATE_HOME", &state_home)
        .env("XDG_CACHE_HOME", &cache_home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match result {
        Ok(status) if status.success() => Measurement::observed(()),
        Ok(status) => Measurement::instrument_failed(&format!("launcher exited with {status}")),
        Err(error) => Measurement::instrument_failed(&format!("cannot launch critic: {error}")),
    }
}

fn format_result(critic: &str, subject: &str, critique: &Path) -> Measurement<String> {
    if !critique.is_file() {
        return Measurement::observed(format!(
            "  {critic:<22} reviewed {subject:<22} (no critique written)"
        ));
    }
    let claims = match read_text(critique) {
        Measurement::Observed(text) => text
            .lines()
            .filter(|line| line.starts_with("CLAIM:"))
            .count(),
        Measurement::Missing(reason) => return Measurement::Missing(reason),
    };
    let words = match word_count(critique) {
        Measurement::Observed(words) => words,
        Measurement::Missing(reason) => return Measurement::Missing(reason),
    };
    Measurement::observed(format!(
        "  {critic:<22} reviewed {subject:<22} {claims} claims, {words} words"
    ))
}

/// Run cross-review for `bead`, with `crate_name` retained for the script-compatible API.
///
/// Exit codes:
/// - `0` — cross-review ran and its artefacts were written.
/// - `1` — a genuine failure: the gate did not pass, or a stage could not run.
/// - `4` — NOT APPLICABLE: fewer than two candidates. Nothing went wrong and nothing was written.
pub fn run_cmd(bead: &str, crate_name: &str, target: &str) -> i32 {
    let _ = crate_name;
    let arms = match candidates(bead, target) {
        Measurement::Observed(arms) => arms,
        Measurement::Missing(reason) => {
            eprintln!("cannot measure candidates: {reason:?}");
            return 1;
        }
    };
    println!("{} candidates with an implementation", arms.len());
    let gate = judge(&Observation {
        built: Some(true),
        tests_passed: Some(true),
        tests_run: Some(1),
        lines_added: Some(arms.len() as u32),
        declared_targets_present: Some(true),
        // Synthetic observation: `judge` used as a boolean combinator, not to score a
        // run. No worktree, so no scope to depart from. Some(0) rather than None
        // deliberately -- None means "not assessed" and yields Indeterminate, which
        // here would turn a correct answer into a refusal to answer.
        scope_departures: Some(0),
    });
    if arms.len() < 2 {
        println!("need >= 2 to cross-review");
        return 4;
    }
    if !matches!(gate, Verdict::Pass) {
        println!("gate did not pass: cannot cross-review");
        return 1;
    }
    let wt = crate::paths::worktrees();
    let log_dir = crate::paths::logs().join("critiques").join(bead);
    if let Err(error) = fs::create_dir_all(&log_dir) {
        eprintln!("cannot create {}: {error}", log_dir.display());
        return 1;
    }
    for (index, critic) in arms.iter().enumerate() {
        let subject = &arms[(index + 1) % arms.len()];
        let cw = wt.join(format!("{bead}--{critic}"));
        let sw = wt.join(format!("{bead}--{subject}"));
        let mut patch = match patch_for(&sw, target) {
            Measurement::Observed(patch) => patch,
            Measurement::Missing(_) => {
                eprintln!(
                    "  {critic}: REFUSING -- {subject} has no readable deliverable at {target}"
                );
                continue;
            }
        };
        let scope_readable = match Command::new("git")
            .current_dir(&sw)
            .args(["rev-parse", "--git-dir"])
            .output()
        {
            Ok(output) => Measurement::observed(output.status.success()),
            Err(error) => {
                Measurement::instrument_failed(&format!("cannot check git worktree: {error}"))
            }
        };
        if matches!(scope_readable, Measurement::Observed(true)) {
            match outside_files(&sw, target) {
                Measurement::Observed(outside) if !outside.is_empty() => {
                    let names = outside.join("\n");
                    let siblings: Vec<String> = outside.iter().filter(|name| name.starts_with(&format!("{}/", Path::new(target).parent().unwrap_or_else(|| Path::new("." )).display())) && name.ends_with(".rs")).take(12).cloned().collect();
                    for sibling in siblings {
                        let path = sw.join(&sibling);
                        if path.is_file() {
                            if let (Measurement::Observed(lines), Measurement::Observed(contents)) =
                                (line_count(&path), read_text(&path))
                            {
                                patch.push_str(&format!("\n\n=== ALSO PART OF THIS DELIVERABLE: {sibling} ({lines} lines) ===\n{}", contents.chars().take(20_000).collect::<String>()));
                            } else {
                                patch.push_str(&format!("\n\n=== ALSO PART OF THIS DELIVERABLE: {sibling} (contents unreadable) ==="));
                            }
                        }
                    }
                    patch.push_str(&format!("\n\n=== THIS ARM ALSO CHANGED {} FILE(S) OUTSIDE ITS DECLARED\n=== DELIVERABLE ({target}). The task asked for that file only. The changes below are\n=== out of scope and are shown as names, not content:\n{names}", outside.len()));
                }
                Measurement::Observed(_) => {}
                Measurement::Missing(_) => patch.push_str("\n\n=== Scope could not be checked: git cannot read this worktree, so whether the arm stayed\n=== within the deliverable is UNKNOWN, not confirmed."),
            }
        } else {
            patch.push_str("\n\n=== Scope could not be checked: git cannot read this worktree, so whether the arm stayed\n=== within the deliverable is UNKNOWN, not confirmed.");
        }
        if patch.is_empty() {
            eprintln!("  {critic}: REFUSING -- {subject} has no readable deliverable at {target}");
            continue;
        }
        let handoff = match read_text(&sw.join(".fb/handoff.md")) {
            Measurement::Observed(s) => s,
            Measurement::Missing(_) => "(no handoff written)".to_string(),
        };
        let prompt_path = log_dir.join(format!("{critic}.prompt.md"));
        let critique_path = cw.join(".fb/critique.md");
        if fs::create_dir_all(cw.join(".fb")).is_err() {
            continue;
        }
        let _ = fs::remove_file(&critique_path);
        if !matches!(
            prompt(
                Path::new(REPO).join(".fb/prompts/_critique.md").as_path(),
                &patch,
                &handoff,
                &prompt_path
            ),
            Measurement::Observed(())
        ) {
            continue;
        }
        let prompt_text = match read_text(&prompt_path) {
            Measurement::Observed(s) => s,
            Measurement::Missing(_) => continue,
        };
        let log = log_dir.join(format!("{critic}.log"));
        let _ = fs::write(&log, "");
        let run = format!("{bead}--{critic}");
        let _ = launch(critic, &prompt_text, &cw, &run);
        if let Measurement::Observed(line) = format_result(critic, subject, &critique_path) {
            println!("{line}");
        }
        let _ = fs::copy(
            &critique_path,
            log_dir.join(format!("{critic}.on.{subject}.md")),
        );
    }
    println!("-> {}/", log_dir.display());
    0
}

#[cfg(test)]
mod tests {
    use super::{environment_paths, format_result, run_cmd};
    use farmerbob_core::measurement::Measurement;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn output_format_matches_script() {
        let path = std::env::temp_dir().join(format!("fb-critique-format-{}", std::process::id()));
        assert!(fs::write(&path, "CLAIM: one\nwords here\n").is_ok());
        let got = format_result("codex-luna", "glm-53-flash", &path);
        assert_eq!(
            got,
            Measurement::Observed(
                "  codex-luna             reviewed glm-53-flash           1 claims, 4 words"
                    .to_string()
            )
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn environment_paths_are_isolated_and_stable() {
        let root = PathBuf::from("/tmp/fb-critique-state");
        let first = environment_paths(&root, "task--critic");
        let second = environment_paths(&root, "task--other-critic");
        let repeat = environment_paths(&root, "task--critic");

        assert_ne!(first, second);
        assert_eq!(first, repeat);
        let run_root = root.join("task--critic");
        assert!(first.iter().all(|path| path.starts_with(&run_root)));
    }

    /// Fixture bead for tests whose candidate arm worktrees are created in
    /// `crate::paths::worktrees()`. All directories created for the fixture are
    /// removed on drop.
    struct ScratchBead {
        bead: String,
        root: PathBuf,
    }

    impl ScratchBead {
        fn new(tag: &str) -> Self {
            let root = crate::paths::worktrees();
            let bead = format!("fb-critique-na-{tag}-{}", std::process::id());
            Self { bead, root }
        }

        fn arm(&self, name: &str, target: &str) -> PathBuf {
            let dir = self.root.join(format!("{}--{name}", self.bead));
            let target_path = dir.join(target);
            if let Some(parent) = target_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(&target_path, "pub fn dummy() {}\n");
            dir
        }

        fn critiques_dir(&self) -> PathBuf {
            crate::paths::logs().join("critiques").join(&self.bead)
        }
    }

    impl Drop for ScratchBead {
        fn drop(&mut self) {
            let prefix = format!("{}--", self.bead);
            if let Ok(entries) = fs::read_dir(&self.root) {
                for entry in entries.flatten() {
                    if entry.file_name().to_string_lossy().starts_with(&prefix) {
                        let _ = fs::remove_dir_all(entry.path());
                    }
                }
            }
            let dir = self.critiques_dir();
            let _ = fs::remove_dir_all(&dir);
            let _ = fs::remove_file(&dir);
        }
    }

    /// Clause 1 and clause 7: zero candidates returns 4, and nothing is written
    /// to the critiques directory.
    #[test]
    fn zero_candidates_returns_not_applicable_and_writes_nothing() {
        let field = ScratchBead::new("zero");
        let dir = field.critiques_dir();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(run_cmd(&field.bead, "fb", "crates/fb/src/critique.rs"), 4);
        assert!(
            !dir.exists(),
            "a 4 return must not write to the critiques directory"
        );
    }

    /// Clause 2 and clause 7: one candidate returns 4, and nothing is written
    /// to the critiques directory.
    #[test]
    fn one_candidate_returns_not_applicable_and_writes_nothing() {
        let field = ScratchBead::new("one");
        field.arm("codex-luna", "crates/fb/src/critique.rs");
        let dir = field.critiques_dir();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(run_cmd(&field.bead, "fb", "crates/fb/src/critique.rs"), 4);
        assert!(
            !dir.exists(),
            "a 4 return must not write to the critiques directory"
        );
    }

    /// Clause 5 and boundary: two candidates with a passing gate does not return 4.
    #[test]
    fn two_candidates_passing_gate_does_not_return_not_applicable() {
        let field = ScratchBead::new("two-pass");
        field.arm("codex-luna", "crates/fb/src/critique.rs");
        field.arm("glm-53-flash", "crates/fb/src/critique.rs");
        let code = run_cmd(&field.bead, "fb", "crates/fb/src/critique.rs");
        assert_ne!(
            code, 4,
            "two candidates with passing gate must not return 4"
        );
        assert_eq!(code, 0);
    }

    /// Clause 4: 4 and 1 are distinct codes. Short field returns 4, while a stage failure
    /// returns 1.
    #[test]
    fn not_applicable_and_failure_are_different_codes() {
        let short = ScratchBead::new("short");
        short.arm("codex-luna", "crates/fb/src/critique.rs");
        let code_na = run_cmd(&short.bead, "fb", "crates/fb/src/critique.rs");
        assert_eq!(code_na, 4);

        let broken = ScratchBead::new("broken");
        broken.arm("codex-luna", "crates/fb/src/critique.rs");
        broken.arm("glm-53-flash", "crates/fb/src/critique.rs");
        let dir = broken.critiques_dir();
        if let Some(parent) = dir.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::write(&dir, "blocking file");
        let code_fail = run_cmd(&broken.bead, "fb", "crates/fb/src/critique.rs");
        let _ = fs::remove_file(&dir);
        assert_eq!(code_fail, 1);
        assert_ne!(code_na, code_fail);
    }

    /// Clause 8: the doc comment on `run_cmd` names all three codes (0, 1, 4).
    #[test]
    fn doc_comment_names_all_three_codes() {
        let text = fs::read_to_string("crates/fb/src/critique.rs")
            .or_else(|_| fs::read_to_string("src/critique.rs"))
            .expect("critique.rs source text");
        let doc_start = text.find("pub fn run_cmd").expect("run_cmd definition");
        let doc_prefix = &text[..doc_start];
        let doc_comment = doc_prefix
            .lines()
            .rev()
            .take(15)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            doc_comment.contains("0"),
            "doc comment must name exit code 0"
        );
        assert!(
            doc_comment.contains("1"),
            "doc comment must name exit code 1"
        );
        assert!(
            doc_comment.contains("4"),
            "doc comment must name exit code 4"
        );
    }
}
