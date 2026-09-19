//! Cross-review an implementation from another implementation's worktree.
//!
//! This is the Rust counterpart of `fb-critique.sh`.  I/O which can fail is kept in
//! [`Measurement`] so that a failed instrument cannot become an observed zero or an empty
//! patch.

use farmerbob_core::gate::{Observation, Verdict, judge};
use farmerbob_core::measurement::Measurement;
use farmerbob_core::stage_cast::{Casting, Stage, Uncast, cast};
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

/// Render the critic's prompt to `prompt_path`, telling it to write its review to
/// `out`.
///
/// These are TWO different paths and were one until 2026-09-19. `{OUT}` was
/// substituted with the prompt's own path, so every critic was instructed to write
/// its review over the prompt it had just been given. They did. The harness then
/// looked for `.fb/critique.md`, found nothing, and printed "(no critique written)"
/// -- while the review sat in the .prompt.md file, complete and unread. Every
/// The first `limit` characters, and when there are more, a marker saying so.
///
/// The marker is the whole point: a reviewer that cannot see the end of a patch must not
/// conclude the end is missing.
fn truncated(text: &str, limit: usize) -> String {
    let head: String = text.chars().take(limit).collect();
    let total = text.chars().count();
    if total <= limit {
        return head;
    }
    format!(
        "{head}\n\n=== PATCH TRUNCATED: you have been shown the first {limit} of {total} \
characters. {} characters were NOT shown. Do NOT conclude that anything is absent from this \
patch -- tests, functions or otherwise. If a claim would depend on what is missing, say that \
you could not see it.",
        total - limit
    )
}

/// critique this stage has ever run was reported as absent.
fn prompt(
    template: &Path,
    patch: &str,
    handoff: &str,
    prompt_path: &Path,
    out: &Path,
) -> Measurement<()> {
    let template = match read_text(template) {
        Measurement::Observed(text) => text,
        Measurement::Missing(reason) => return Measurement::Missing(reason),
    };
    let text = template
        // A SILENT truncation makes the harness manufacture false accusations. cost-honesty's
        // patch was 76,372 chars; the critic saw the first 60,000, could not see the tests in
        // the last 16KB, and reported them "absent from the patch diff". It was not wrong
        // about what it was shown -- nothing told it there was more. An arm was accused of
        // claiming tests it had in fact written.
        .replace("{PATCH}", &truncated(patch, 60_000))
        .replace(
            "{HANDOFF}",
            &handoff.chars().take(4_000).collect::<String>(),
        )
        .replace("{OUT}", &out.to_string_lossy());
    match fs::write(prompt_path, text) {
        Ok(()) => Measurement::observed(()),
        Err(error) => Measurement::instrument_failed(&format!(
            "cannot write {}: {error}",
            prompt_path.display()
        )),
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

fn launch(arm: &str, text: &str, worktree: &Path, run: &str, log: &Path) -> Measurement<()> {
    // A test that reaches the launcher spends money and hangs. Before critics could be drawn
    // from the registry no test could get this far -- a one-arm field returned 4 first -- and
    // the moment that changed, `not_applicable_and_failure_are_different_codes` launched a
    // real agent from `cargo test`. The stage still reports the assignment, as "(no critique
    // written)", which is exactly what it reports for a launcher that failed in production.
    if cfg!(test) {
        return Measurement::instrument_failed("launcher disabled under cargo test");
    }
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
        .output();
    // Capture what the launcher said. It was Stdio::null() on both streams while an empty
    // <critic>.log was written beside it, so a critic that crashed on startup and a critic
    // that ran and chose to write nothing produced the identical artefact: a zero-byte log
    // and "(no critique written)". There was no way to tell them apart, which is why the
    // first three runs of this stage could not be diagnosed at all.
    if let Ok(ref out) = result {
        let mut captured = String::from_utf8_lossy(&out.stdout).into_owned();
        let err = String::from_utf8_lossy(&out.stderr);
        if !err.is_empty() {
            captured.push_str("\n=== stderr ===\n");
            captured.push_str(&err);
        }
        if captured.is_empty() {
            captured.push_str("(the launcher wrote nothing to stdout or stderr)\n");
        }
        let _ = fs::write(log, captured);
    }
    let result = result.map(|out| out.status);
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

/// Every arm this harness may cast as a critic, in `sources.toml` order.
///
/// A critic does not need a candidate's worktree and does not need to have
/// implemented anything: it reads the deliverable from its prompt. So the roster
/// is the REGISTRY, not the field. That distinction is the whole of this stage --
/// `crossx` needs two implementations and this never did.
fn roster() -> Measurement<Vec<String>> {
    // paths::repo() honours FB_REPO and falls back to the working directory, which is not
    // the repository root under `cargo test`. Fall back to the same constant candidates()
    // has always used rather than making the roster unreadable from a test.
    let mut path = crate::paths::repo().join("sources.toml");
    if !path.is_file() {
        path = Path::new(REPO).join("sources.toml");
    }
    match crate::sources::Registry::load(&path) {
        Ok(registry) => {
            // Free arms first, registry order within each group. stage_cast casts the FIRST
            // roster entry that is not the subject, so this ordering is what decides who
            // pays for the review -- and the first run of this stage cast agy-opus-46,
            // purely because it sits early in sources.toml.
            let all = registry.dispatchable();
            let mut free: Vec<String> = Vec::new();
            let mut paid: Vec<String> = Vec::new();
            for (name, source) in all {
                if source.is_free() {
                    free.push(name.to_string());
                } else {
                    paid.push(name.to_string());
                }
            }
            free.extend(paid);
            Measurement::observed(free)
        }
        Err(error) => Measurement::instrument_failed(&error),
    }
}

/// Who reviews whom, or the exit code to return instead.
///
/// Split out from [`run_cmd`] so it can be tested without launching an agent. A
/// test that reached the launcher would spend money and hang, which is why the
/// test this replaces could only ever assert the not-applicable path.
fn plan(arms: &[String], roster: &[String]) -> Result<Vec<Casting>, i32> {
    match cast(Stage::Critique, arms, roster) {
        Ok(castings) => Ok(castings),
        // Nothing to examine. Nothing went wrong and nothing is written.
        Err(Uncast::TooFewCandidates { needed, have }) => {
            println!("no candidates to critique (need {needed}, have {have})");
            Err(4)
        }
        // There IS work to examine and nobody free to examine it. A staffing failure is a
        // refusal, not a not-applicable, and it must not be reported as one.
        Err(Uncast::NoIndependentArm) => {
            eprintln!(
                "no arm in the registry can review this field: every eligible arm authored it"
            );
            Err(1)
        }
    }
}

/// Where a critic works.
///
/// A critic that is also a candidate keeps its own worktree, which is what this
/// stage has always done. A critic drawn from the roster has none, so it gets a
/// scratch one off the repository's current HEAD. It is never given the SUBJECT's
/// worktree: that is the artefact under examination, and a critic writing into it
/// would corrupt the thing being measured.
fn critic_worktree(
    wt: &Path,
    bead: &str,
    critic: &str,
    candidates: &[String],
) -> Measurement<PathBuf> {
    let own = wt.join(format!("{bead}--{critic}"));
    if candidates.iter().any(|c| c == critic) && own.is_dir() {
        return Measurement::observed(own);
    }
    let scratch = wt.join(format!("{bead}--critic--{critic}"));
    if scratch.is_dir() {
        return Measurement::observed(scratch);
    }
    let repo = crate::paths::repo();
    let status = Command::new("git")
        .current_dir(&repo)
        .args(["worktree", "add", "--detach", "--force"])
        .arg(&scratch)
        .arg("HEAD")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(status) if status.success() => Measurement::observed(scratch),
        Ok(status) => Measurement::instrument_failed(&format!(
            "cannot create a worktree for critic {critic}: git exited with {status}"
        )),
        Err(error) => Measurement::instrument_failed(&format!(
            "cannot create a worktree for critic {critic}: {error}"
        )),
    }
}

/// Critique every candidate for `bead`; `crate_name` is retained for the
/// script-compatible API.
///
/// Critics come from the REGISTRY, not the field: reviewing a deliverable needs a
/// second agent, not a second implementation. One candidate is a normal field.
///
/// Exit codes:
/// - `0` — every assignment was ATTEMPTED. A critic whose launcher fails is a gap
///   in the evidence, recorded as "(no critique written)", not a failed stage.
/// - `1` — the gate did not pass, the roster is unreadable, or there is work to
///   examine and no arm free to examine it.
/// - `4` — NOT APPLICABLE: no candidates. Nothing went wrong, nothing was written.
pub fn run_cmd(bead: &str, crate_name: &str, target: &str) -> i32 {
    let _ = crate_name;
    let arms = match candidates(bead, target) {
        Measurement::Observed(arms) => arms,
        Measurement::Missing(reason) => {
            eprintln!("cannot measure candidates: {reason:?}");
            return 1;
        }
    };
    println!("{} candidate(s) with an implementation", arms.len());
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
    // WHO REVIEWS. Critics come from the registry, not from the field.
    //
    // Until this change the assignment was `arms[(index + 1) % arms.len()]` -- a ring over the
    // CANDIDATES -- so a one-arm field returned 4 "need >= 2 to cross-review", and promote and
    // prove returned 4 in turn for want of critiques. The whole subjective tier went dark on
    // every single-arm task, which is now every task. But critique compares nothing: it puts a
    // model in front of a deliverable and asks what is wrong with it, and that needs a second
    // AGENT, not a second IMPLEMENTATION. `crossx` is the stage that genuinely needs two.
    //
    // farmerbob_core::stage_cast makes that distinction and this is its first caller.
    let roster = match roster() {
        Measurement::Observed(roster) => roster,
        // An unreadable registry is a failure only if there is something to review. With an
        // empty field the registry is irrelevant, and reporting 1 here would convert
        // "nothing to review" into "the stage broke" -- the substitution this whole project
        // exists to prevent.
        Measurement::Missing(_) if arms.is_empty() => Vec::new(),
        Measurement::Missing(reason) => {
            eprintln!("cannot read the roster: {reason:?}");
            return 1;
        }
    };
    let castings = match plan(&arms, &roster) {
        Ok(castings) => castings,
        Err(code) => return code,
    };
    // The gate is checked AFTER applicability, and the order matters: an empty field adds no
    // lines, so judging it first turns "nothing to review" into "the gate failed" -- which is
    // how this read 1 instead of 4 the first time the two were swapped.
    if !matches!(gate, Verdict::Pass) {
        println!("gate did not pass: cannot critique");
        return 1;
    }
    let wt = crate::paths::worktrees();
    let log_dir = crate::paths::logs().join("critiques").join(bead);
    if let Err(error) = fs::create_dir_all(&log_dir) {
        eprintln!("cannot create {}: {error}", log_dir.display());
        return 1;
    }
    for casting in &castings {
        let critic = &casting.arm;
        // Stage::Critique always names a subject; stage_cast pins that. A casting without one
        // would be a SpecCritic seat, which this command does not run.
        let Some(subject) = casting.subject.as_ref() else {
            continue;
        };
        let cw = match critic_worktree(&wt, bead, critic, &arms) {
            Measurement::Observed(path) => path,
            Measurement::Missing(reason) => {
                eprintln!("  {critic}: REFUSING -- {reason:?}");
                continue;
            }
        };
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
                &prompt_path,
                &critique_path
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
        let run = format!("{bead}--{critic}");
        if let Measurement::Missing(reason) = launch(critic, &prompt_text, &cw, &run, &log) {
            // `let _ =` here discarded this for as long as the stage existed, so a critic
            // that never started and a critic that started and wrote nothing produced the
            // identical line: "(no critique written)".
            eprintln!("  {critic}: launcher did not complete -- {reason:?}");
        }
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
    fn one_candidate_is_reviewable_by_an_arm_from_the_registry() {
        // The inverse of the test this replaces. `one_candidate_returns_not_applicable`
        // asserted 4, and that assertion was the whole reason the subjective tier stayed
        // dark on every one-arm field: critique returned 4, so promote had no critiques,
        // so prove had no claims. One candidate is a normal, reviewable field.
        let arms = vec!["glm-53-flash".to_string()];
        let roster = vec!["glm-53-flash".to_string(), "codex-luna".to_string()];
        let castings = super::plan(&arms, &roster).expect("one candidate is reviewable");
        assert_eq!(castings.len(), 1);
        assert_eq!(
            castings[0].arm, "codex-luna",
            "the critic is not the author"
        );
        assert_eq!(castings[0].subject.as_deref(), Some("glm-53-flash"));
    }

    /// Zero candidates is still not-applicable, and it is the ONLY not-applicable.
    #[test]
    fn zero_candidates_is_the_only_not_applicable() {
        let roster = vec!["codex-luna".to_string()];
        assert_eq!(super::plan(&[], &roster), Err(4));
    }

    /// Work to examine and nobody free to examine it is a refusal, not a
    /// not-applicable. Same field as the test above, one entry removed from the
    /// roster: the two answers must be different.
    #[test]
    fn no_independent_arm_is_a_refusal_not_not_applicable() {
        let arms = vec!["codex-luna".to_string()];
        let roster = vec!["codex-luna".to_string()];
        assert_eq!(super::plan(&arms, &roster), Err(1));
    }

    /// Two candidates still review each other: the ring falls out of
    /// "first roster entry that is not the subject" rather than being coded.
    #[test]
    fn two_candidates_still_review_each_other() {
        let arms = vec!["a".to_string(), "b".to_string()];
        let roster = vec!["a".to_string(), "b".to_string()];
        let castings = super::plan(&arms, &roster).expect("two candidates are reviewable");
        assert_eq!(castings.len(), 2);
        assert_eq!(castings[0].arm, "b");
        assert_eq!(castings[0].subject.as_deref(), Some("a"));
        assert_eq!(castings[1].arm, "a");
        assert_eq!(castings[1].subject.as_deref(), Some("b"));
    }

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

    /// 4 and 1 are distinct codes, and the short field that yields 4 is now the EMPTY
    /// one. This test asserted that a ONE-arm field returns 4; that assertion was the
    /// contract this change exists to break.
    #[test]
    fn not_applicable_and_failure_are_different_codes() {
        let empty = ScratchBead::new("short");
        let code_na = run_cmd(&empty.bead, "fb", "crates/fb/src/critique.rs");
        assert_eq!(code_na, 4, "an empty field is not applicable");

        let lone = ScratchBead::new("lone");
        lone.arm("codex-luna", "crates/fb/src/critique.rs");
        let code_one = run_cmd(&lone.bead, "fb", "crates/fb/src/critique.rs");
        assert_ne!(
            code_one, 4,
            "one candidate is reviewable by an arm from the registry"
        );
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
