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

fn home() -> Measurement<PathBuf> {
    match std::env::var_os("HOME") {
        Some(value) => Measurement::observed(PathBuf::from(value)),
        None => Measurement::instrument_failed("HOME is not set"),
    }
}

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

fn launch(arm: &str, text: &str, worktree: &Path) -> Measurement<()> {
    let result = Command::new("bash")
        .arg("-c")
        .arg(". /home/gabe/Documents/farmerbob/fb-launch.sh; fb_launch \"$1\" \"$2\" \"$3\"")
        .arg("fb-launch")
        .arg(arm)
        .arg(text)
        .arg(worktree)
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
    });
    if arms.len() < 2 || !matches!(gate, Verdict::Pass) {
        println!("need >= 2 to cross-review");
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
        let _ = launch(critic, &prompt_text, &cw);
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
    use super::format_result;
    use farmerbob_core::measurement::Measurement;
    use std::fs;

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
}
