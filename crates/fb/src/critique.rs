//! Cross-review a deliverable with a critic drawn from the roster.
//!
//! This is the Rust counterpart of `fb-critique.sh`.  I/O which can fail is kept in
//! [`Measurement`] so that a failed instrument cannot become an observed zero or an empty
//! patch.
//!
//! Critique compares nothing: it puts a model in front of a deliverable and asks what is
//! wrong with it. That needs a second AGENT, not a second IMPLEMENTATION, so the critics
//! come from the roster -- every arm in `sources.toml` whose status is exactly `verified`
//! or `untested`, in file order -- while the field supplies only the subjects. The report
//! filed under `logs/critiques/<bead>/<critic>.on.<subject>.md` names both arms, so the
//! credit for finding what nobody else saw survives past the terminal. `crossx` is the
//! stage that genuinely needs two implementations; this one never did.

use farmerbob_core::gate::{Observation, Verdict, judge};
use farmerbob_core::measurement::Measurement;
use farmerbob_core::stage_cast::{Casting, Stage, Uncast, cast};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

/// The field: worktrees named `<bead>--<arm>` that hold a non-empty deliverable
/// at `target` and no task file.
///
/// The measurement is the FIELD, not the registry. Registry status decides who may
/// EXAMINE -- that is [`roster`]'s job -- and says nothing about who may be examined: a
/// candidate absent from the roster is still a valid subject, which is the distinction
/// that keeps "an empty roster" and "no candidates" different answers instead of two
/// spellings of one. An empty root is an empty field and needs no registry to say so; an
/// unreadable one is an instrument failure.
fn candidates(bead: &str, target: &str) -> Measurement<Vec<String>> {
    let root = crate::paths::worktrees();
    let prefix = format!("{bead}--");
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

/// Render the critic's prompt to `prompt_path`, telling it to write its review to
/// `out`.
///
/// These are TWO different paths and were one until 2026-09-19. `{OUT}` was
/// substituted with the prompt's own path, so every critic was instructed to write
/// its review over the prompt it had just been given. They did. The harness then
/// looked for `.fb/critique.md`, found nothing, and printed "(no critique written)"
/// -- while the review sat in the .prompt.md file, complete and unread. Every
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
    // A test that reaches the launcher spends money and hangs.
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
    // crate::launch, not fb-launch.sh. One launcher table, in Rust, tested.
    let env = [
        ("XDG_DATA_HOME", data_home),
        ("XDG_STATE_HOME", state_home),
        ("XDG_CACHE_HOME", cache_home),
    ];
    match crate::launch::launch(arm, text, worktree, false, &env) {
        Measurement::Observed(said) => {
            let _ = fs::write(
                log,
                if said.is_empty() {
                    "(the launcher wrote nothing to stdout or stderr)\n".to_string()
                } else {
                    said
                },
            );
            Measurement::observed(())
        }
        Measurement::Missing(reason) => {
            let _ = fs::write(log, format!("{reason:?}\n"));
            Measurement::Missing(reason)
        }
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

/// A file from the repository root: `FB_REPO` when it holds one, else the same
/// constant this stage has always fallen back to.
///
/// `paths::repo()` itself falls back to the working directory, which is not the
/// repository root under `cargo test`, so the constant keeps a test without a sandboxed
/// repository from seeing an empty roster.
fn repo_file(relative: &str) -> PathBuf {
    // NO FALLBACK. This returned the hard-coded repository whenever the configured path was
    // not a file, so an `FB_REPO` pointing at a tree with no `sources.toml` silently read
    // the REAL roster and cast critics from it, reporting success.
    //
    // `FB_REPO` is the isolation override -- the whole point of `paths::repo` is that a
    // differential or a test can be redirected away from the real artefacts -- and a
    // fallback to the real tree defeats exactly that, while making a misconfigured run
    // indistinguishable from a correct one.
    //
    // The caller reports `Missing` when the file is not there, which is the honest answer.
    // Found by codex-luna critiquing this task.
    crate::paths::repo().join(relative)
}

/// Every arm named by a `[source.<arm>]` header, in the order the headers appear.
///
/// `Registry` parses the file but stores a `BTreeMap`, which cannot express file order --
/// and the order is load-bearing, because stage_cast casts the FIRST roster entry that is
/// not the subject, so the order decides who reviews. A header this scan cannot read
/// yields an arm the registry filter below will never match, so a mis-read arm drops out
/// of the roster rather than being cast unvetted.
fn arm_names_in_file_order(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in text.lines() {
        let line = line.trim_start();
        let Some(rest) = line.strip_prefix("[source.") else {
            continue;
        };
        let Some(end) = rest.find(']') else {
            continue;
        };
        let after = rest[end + 1..].trim();
        if !after.is_empty() && !after.starts_with('#') {
            continue;
        }
        let quoted = rest[..end].trim();
        let name = quoted
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| quoted.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
            .unwrap_or(quoted);
        if !name.is_empty() {
            names.push(name.to_string());
        }
    }
    names
}

/// Every arm this harness may cast as a critic, in `sources.toml` order.
///
/// The roster is the REGISTRY, not the field: a critic does not need a candidate's
/// worktree and does not need to have implemented anything, because it reads the
/// deliverable from its prompt. The statuses accepted are exactly `verified` and
/// `untested` -- anything else, `disabled` included and any status this code has never
/// heard of, is excluded. An unrecognised status must meet the same closed door as a
/// recognised refusal: treating an unknown status as castable is the dangerous direction,
/// and `fb eligible` already refuses it, so the two implementations of one rule must not
/// disagree here. A roster that cannot be read is `Missing`, and the run fails; it is not
/// silently an empty roster.
fn roster() -> Measurement<Vec<String>> {
    let path = repo_file("sources.toml");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            return Measurement::instrument_failed(&format!(
                "cannot read {}: {error}",
                path.display()
            ));
        }
    };
    let registry = match crate::sources::Registry::load(&path) {
        Ok(registry) => registry,
        Err(error) => return Measurement::instrument_failed(&error),
    };
    let roster = arm_names_in_file_order(&text)
        .into_iter()
        .filter(|arm| {
            matches!(
                registry.get(arm).map(|source| source.status.as_str()),
                Some("verified" | "untested")
            )
        })
        .collect();
    Measurement::observed(roster)
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
            eprintln!("no roster arm can review this field: every roster arm authored it");
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
/// Who reviews whom is `stage_cast::cast`'s decision. This command measures the field,
/// reads the roster, makes the worktrees, launches, and records; it decides nothing
/// about pairing. One candidate is a normal, reviewable field.
///
/// Exit codes:
/// - `0` — critiques ran and their artefacts were written: every assignment was
///   ATTEMPTED, in the order `cast` returned them. A critic whose launcher fails is a
///   gap in the evidence, recorded as "(no critique written)", not a failed stage.
/// - `1` — a genuine failure: the gate did not pass, the roster could not be read,
///   or there is work to examine and no arm free to examine it.
/// - `4` — NOT APPLICABLE: zero candidates. Nothing went wrong, nothing was written.
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
    // WHO REVIEWS. Critics come from the roster, not from the field.
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
        // An unreadable roster is a failure only if there is something to review. With an
        // empty field the roster is irrelevant, and reporting 1 here would convert
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
                    let siblings: Vec<String> = outside.iter().filter(|name| name.starts_with(&format!("{}/", Path::new(target).parent().unwrap_or_else(|| Path::new(".")).display())) && name.ends_with(".rs")).take(12).cloned().collect();
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
                &repo_file(".fb/prompts/_critique.md"),
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
        // The report filed under logs/critiques/<bead>/ is what promote, adjudicate and
        // followups read, and its stem is where the critic's credit lives: the critic and
        // the subject are both in the name, so two critics of one subject cannot overwrite
        // one another. A copy that fails is a lost credit, and losing one silently is how
        // this stage used to lose every critique it ever ran.
        if critique_path.is_file() {
            let report = log_dir.join(format!("{critic}.on.{subject}.md"));
            if let Err(error) = fs::copy(&critique_path, &report) {
                eprintln!(
                    "  {critic}: cannot file the critique at {}: {error}",
                    report.display()
                );
            }
        }
    }
    println!("-> {}/", log_dir.display());
    0
}

#[cfg(test)]
mod tests {
    use super::{environment_paths, format_result, run_cmd};
    use farmerbob_core::measurement::Measurement;
    use farmerbob_core::stage_cast::{Stage, Uncast, cast};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

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

    /// Clause 8 of the documented contract: the doc comment on `run_cmd` names all
    /// three codes (0, 1, 4).
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

    // ---- the sandbox ----
    //
    // `run_cmd` resolves its tree through `crate::paths`, which reads the process
    // environment: the seam this stage names for tests is `FB_WT` and `FB_REPO`, with
    // `FB_LOGS` beside them. Those variables are process-global, and other tests in
    // this binary read the REAL registry through `paths::repo()` with no lock of their
    // own -- `fb doctor`'s launcher-coverage check and `fb eligible`'s run_cmd contract
    // both do. A mutex here cannot protect them: an earlier version of this suite held
    // the lock twice on one thread and deadlocked the run, and the lock it did hold did
    // not stop `FB_REPO` from racing those readers. So every sandboxed body below runs
    // in a CHILD COPY of this test binary, filtered to that one test and given the
    // sandbox environment on spawn. The parent process never mutates its own
    // environment and holds no lock, so nothing here can deadlock the suite or
    // redirect another test's reads. No agent can run from a sandbox either: the
    // launcher is disabled under `cargo test`, so every assignment ends as "(no
    // critique written)" and a run that reached its assignments still returns 0.

    /// Marks the child half of a sandboxed test: the parent puts the sandbox root on
    /// the child's environment, and a test that sees the marker runs its body against
    /// the attached sandbox instead of building one.
    const SANDBOX_ROOT: &str = "FB_CRITIQUE_SANDBOX_ROOT";

    /// Carries the bead from parent to child, so the child attaches to the same field
    /// names without re-deriving them from a path.
    const SANDBOX_BEAD: &str = "FB_CRITIQUE_SANDBOX_BEAD";

    fn in_child() -> bool {
        std::env::var_os(SANDBOX_ROOT).is_some()
    }

    fn git(args: &[&str], cwd: &Path) {
        let status = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .status()
            .expect("git should be runnable in the test environment");
        assert!(status.success(), "git {args:?} failed in {}", cwd.display());
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = match fs::read_dir(dir) {
            Ok(entries) => entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().to_string())
                .collect(),
            Err(_) => Vec::new(),
        };
        names.sort();
        names
    }

    const ROSTER_B: &str = r#"
[source.roster-b]
status = "verified"
"#;

    const ROSTER_B_AND_C: &str = r#"
[source.roster-b]
status = "verified"

[source.roster-c]
status = "verified"
"#;

    const ROSTER_Z_FIRST: &str = r#"
[source.roster-z]
status = "verified"

[source.roster-a]
status = "verified"
"#;

    const ROSTER_A_AND_B: &str = r#"
[source.cand-a]
status = "verified"

[source.cand-b]
status = "verified"
"#;

    const ROSTER_ONLY_SUBJECT: &str = r#"
[source.cand-a]
status = "verified"
"#;

    const ROSTER_EMPTY: &str = "";

    const DISABLED_FIRST: &str = r#"
[source.switched-off]
status = "disabled"
disabled_reason = "operator turned it off"

[source.eligible-e]
status = "verified"
"#;

    const UNKNOWN_STATUS_FIRST: &str = r#"
[source.hibernate-h]
status = "hibernating"

[source.eligible-e]
status = "verified"
"#;

    const UNTESTED_ONLY: &str = r#"
[source.fresh-f]
status = "untested"
"#;

    /// A sandboxed field: one temporary root holding a git repository (`repo/`, with the
    /// fixture `sources.toml` and the critic prompt template), a git worktree root
    /// (`wt/`), and a logs root whose `critiques/<bead>/` receives the run's artefacts.
    struct Sandbox {
        root: PathBuf,
        bead: String,
        target: &'static str,
    }

    impl Sandbox {
        /// Parent side: build the sandbox and its two git repositories. No
        /// environment is touched -- the child gets it on spawn.
        fn build(tag: &str, sources_toml: &str) -> Sandbox {
            let root =
                std::env::temp_dir().join(format!("fb-critique-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("repo/.fb/prompts")).expect("create the repo sandbox");
            fs::create_dir_all(root.join("wt")).expect("create the worktree root");
            fs::write(root.join("repo/sources.toml"), sources_toml)
                .expect("write the fixture sources.toml");
            fs::write(
                root.join("repo/.fb/prompts/_critique.md"),
                "review the patch:\n\n{PATCH}\n\nwrite your critique to {OUT}\n",
            )
            .expect("write the prompt template");
            let repo = root.join("repo");
            git(&["init", "-q"], &repo);
            git(&["config", "user.email", "fb@example.com"], &repo);
            git(&["config", "user.name", "fb"], &repo);
            git(&["add", "-A"], &repo);
            git(&["commit", "-qm", "sandbox", "--no-verify"], &repo);
            let wt = root.join("wt");
            git(&["init", "-q"], &wt);
            git(&["config", "user.email", "fb@example.com"], &wt);
            git(&["config", "user.name", "fb"], &wt);
            git(
                &["commit", "-qm", "sandbox", "--allow-empty", "--no-verify"],
                &wt,
            );
            Sandbox {
                root,
                bead: tag.to_string(),
                target: "deliv.rs",
            }
        }

        /// Child side: attach to the sandbox the parent built and passed down in the
        /// environment. Nothing is rebuilt and nothing is cleaned up here -- the
        /// parent owns the sandbox's lifetime.
        fn attached() -> Sandbox {
            let root = PathBuf::from(std::env::var_os(SANDBOX_ROOT).expect("sandbox root env"));
            let bead = std::env::var(SANDBOX_BEAD).expect("sandbox bead env");
            Sandbox {
                root,
                bead,
                target: "deliv.rs",
            }
        }

        /// Parent side: run this test again in a child copy of the test binary,
        /// filtered to exactly that test, with the sandbox environment attached.
        /// Returns whether the child actually ran the one test and passed, and
        /// removes the sandbox either way. A filter that matches nothing would
        /// otherwise exit 0 and pass vacuously, so the child's own summary is
        /// checked rather than trusted.
        fn spawn_child(&self, test: &str) -> bool {
            let exe = std::env::current_exe().expect("the test binary path");
            // `module_path!()` inside a bin crate carries the crate name --
            // "fb::critique::tests" -- while libtest names unit tests from the crate
            // root -- "critique::tests::X". Strip the first segment, whatever the
            // module this suite is compiled into, so the child's filter is a name
            // libtest actually knows.
            let module = module_path!()
                .split("::")
                .skip(1)
                .collect::<Vec<_>>()
                .join("::");
            let name = format!("{module}::{test}");
            let output = Command::new(exe)
                .args(["--exact", &name, "--test-threads", "1"])
                .env(SANDBOX_ROOT, self.root.as_os_str())
                .env(SANDBOX_BEAD, self.bead.as_str())
                .env("FB_WT", self.wt().as_os_str())
                .env("FB_REPO", self.repo().as_os_str())
                .env("FB_LOGS", self.root.join("logs").as_os_str())
                .env("FB_STATE", self.root.as_os_str())
                .output()
                .expect("spawn the test binary");
            let _ = fs::remove_dir_all(&self.root);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let ran = stdout.contains("ok. 1 passed") && output.status.success();
            if !ran {
                eprint!("{}", String::from_utf8_lossy(&output.stderr));
            }
            ran
        }

        /// Creates one candidate worktree holding a non-empty deliverable. The arm is
        /// deliberately NOT registered in `sources.toml` unless the fixture says
        /// otherwise: the roster says who may EXAMINE, not who may be examined.
        fn arm(&self, name: &str) -> PathBuf {
            let dir = self.subject_wt(name);
            fs::create_dir_all(&dir).expect("create the candidate worktree");
            fs::write(dir.join(self.target), "pub fn delivered() {}\n")
                .expect("write the deliverable");
            dir
        }

        fn wt(&self) -> PathBuf {
            self.root.join("wt")
        }

        fn repo(&self) -> PathBuf {
            self.root.join("repo")
        }

        fn subject_wt(&self, name: &str) -> PathBuf {
            self.wt().join(format!("{}--{name}", self.bead))
        }

        fn critic_wt(&self, name: &str) -> PathBuf {
            self.wt().join(format!("{}--critic--{name}", self.bead))
        }

        fn critiques_dir(&self) -> PathBuf {
            self.root.join("logs").join("critiques").join(&self.bead)
        }

        fn set_sources(&self, sources_toml: &str) {
            fs::write(self.repo().join("sources.toml"), sources_toml)
                .expect("rewrite the fixture sources.toml");
        }
    }

    /// Clause 1, and the inverse of the test it replaces: ONE candidate with a roster
    /// holding other arms is a normal, reviewable field. It produces ONE critique
    /// assignment -- the first roster arm that is not the subject gets the one scratch
    /// worktree the stage pins at `<worktrees>/<bead>--critic--<arm>` -- and it does NOT
    /// return 4. `one_candidate_returns_not_applicable_and_writes_nothing` asserted the
    /// opposite, and that assertion was the whole reason the subjective tier went dark on
    /// every one-arm field: critique returned 4, so promote had no critiques, so prove
    /// had no claims.
    #[test]
    fn one_candidate_returns_zero_and_one_assignment() {
        if in_child() {
            one_candidate_body(&Sandbox::attached());
            return;
        }
        let sb = Sandbox::build("one-cand", ROSTER_B_AND_C);
        sb.arm("cand-a");
        assert!(
            sb.spawn_child("one_candidate_returns_zero_and_one_assignment"),
            "the sandboxed body failed in the child"
        );
    }

    fn one_candidate_body(sb: &Sandbox) {
        let code = run_cmd(&sb.bead, "fb", sb.target);
        assert_eq!(code, 0, "one candidate is a normal, reviewable field");
        assert!(
            sb.critic_wt("roster-b").is_dir(),
            "the first roster arm that is not the subject is cast"
        );
        assert!(
            !sb.critic_wt("roster-c").exists(),
            "only the cast critic gets a working directory"
        );
        assert!(
            !sb.critic_wt("cand-a").exists(),
            "the subject is not one of its own reviewers"
        );
    }

    /// Clause 5: the assignment is stage_cast's, not this file's arithmetic. The oracle
    /// is `cast` called on the same field and roster the run just used; the run's one
    /// observable -- which critic got a working directory -- has to agree with it.
    #[test]
    fn the_assignment_agrees_with_stage_cast() {
        if in_child() {
            assignment_agrees_body(&Sandbox::attached());
            return;
        }
        let sb = Sandbox::build("agree-cast", ROSTER_B);
        sb.arm("cand-a");
        assert!(
            sb.spawn_child("the_assignment_agrees_with_stage_cast"),
            "the sandboxed body failed in the child"
        );
    }

    fn assignment_agrees_body(sb: &Sandbox) {
        let castings = cast(
            Stage::Critique,
            &["cand-a".to_string()],
            &["roster-b".to_string()],
        )
        .expect("one candidate with an independent roster arm casts");
        assert_eq!(castings.len(), 1);
        assert_eq!(castings[0].arm, "roster-b");
        assert_eq!(castings[0].subject.as_deref(), Some("cand-a"));
        assert_eq!(run_cmd(&sb.bead, "fb", sb.target), 0);
        assert!(
            sb.critic_wt("roster-b").is_dir(),
            "the run cast exactly whom stage_cast cast"
        );
    }

    /// Clause 2 and clause 10: zero candidates is NOT APPLICABLE, and it is the empty
    /// FIELD that makes it so -- the worktree root exists and is readable, and the
    /// roster even has a castable arm. Nothing is written.
    #[test]
    fn zero_candidates_returns_not_applicable_and_writes_nothing() {
        if in_child() {
            zero_candidates_body(&Sandbox::attached());
            return;
        }
        let sb = Sandbox::build("zero-cand", ROSTER_B);
        assert!(
            sb.spawn_child("zero_candidates_returns_not_applicable_and_writes_nothing"),
            "the sandboxed body failed in the child"
        );
    }

    fn zero_candidates_body(sb: &Sandbox) {
        assert_eq!(run_cmd(&sb.bead, "fb", sb.target), 4);
        let dir = sb.critiques_dir();
        assert!(
            !dir.exists() || fs::read_dir(&dir).map(|es| es.count()).unwrap_or(0) == 0,
            "a not-applicable run writes nothing"
        );
    }

    /// Clause 3: two candidates with a roster of exactly those two arms still review
    /// each other -- the behaviour before this change, preserved. The ring itself is
    /// gone; the pairing falls out of "the first roster entry that is not the subject".
    #[test]
    fn two_candidates_with_a_roster_of_exactly_those_two_review_each_other() {
        if in_child() {
            two_candidates_body(&Sandbox::attached());
            return;
        }
        let sb = Sandbox::build("two-cand", ROSTER_A_AND_B);
        sb.arm("cand-a");
        sb.arm("cand-b");
        assert!(
            sb.spawn_child("two_candidates_with_a_roster_of_exactly_those_two_review_each_other"),
            "the sandboxed body failed in the child"
        );
    }

    fn two_candidates_body(sb: &Sandbox) {
        assert_eq!(run_cmd(&sb.bead, "fb", sb.target), 0);
        let castings = cast(
            Stage::Critique,
            &["cand-a".to_string(), "cand-b".to_string()],
            &["cand-a".to_string(), "cand-b".to_string()],
        )
        .expect("two candidates cast against their own roster");
        assert_eq!(castings.len(), 2, "one assignment per candidate");
        assert_eq!(castings[0].arm, "cand-b");
        assert_eq!(castings[0].subject.as_deref(), Some("cand-a"));
        assert_eq!(castings[1].arm, "cand-a");
        assert_eq!(castings[1].subject.as_deref(), Some("cand-b"));
    }

    /// Clause 4: a roster whose only entry is the sole candidate is work to examine
    /// with nobody free to examine it -- a staffing failure is a refusal (1), not a
    /// not-applicable (4).
    #[test]
    fn a_roster_of_only_the_subject_is_a_refusal_not_not_applicable() {
        if in_child() {
            roster_only_subject_body(&Sandbox::attached());
            return;
        }
        let sb = Sandbox::build("lone-cand", ROSTER_ONLY_SUBJECT);
        sb.arm("cand-a");
        assert!(
            sb.spawn_child("a_roster_of_only_the_subject_is_a_refusal_not_not_applicable"),
            "the sandboxed body failed in the child"
        );
    }

    fn roster_only_subject_body(sb: &Sandbox) {
        let code = run_cmd(&sb.bead, "fb", sb.target);
        assert_eq!(code, 1, "work to examine, nobody free to examine it");
        assert_ne!(code, 4, "a staffing failure is not a not-applicable");
    }

    /// The boundary beside clause 4: an outright empty roster with one candidate is
    /// the same answer for the same reason -- `cast` refuses with `NoIndependentArm`,
    /// which run_cmd reports as 1.
    #[test]
    fn an_empty_roster_with_one_candidate_is_the_same_refusal() {
        if in_child() {
            empty_roster_body(&Sandbox::attached());
            return;
        }
        let sb = Sandbox::build("empty-roster", ROSTER_EMPTY);
        sb.arm("cand-a");
        assert!(
            sb.spawn_child("an_empty_roster_with_one_candidate_is_the_same_refusal"),
            "the sandboxed body failed in the child"
        );
    }

    fn empty_roster_body(sb: &Sandbox) {
        assert_eq!(
            run_cmd(&sb.bead, "fb", sb.target),
            1,
            "an empty roster with one candidate is the same refusal"
        );
        assert_eq!(
            cast(Stage::Critique, &["cand-a".to_string()], &[]),
            Err(Uncast::NoIndependentArm)
        );
    }

    /// The roster is in `sources.toml` order, not sorted order: stage_cast casts the
    /// first roster entry that is not the subject, so an implementation that lets the
    /// registry's BTreeMap decide would cast `roster-a` where the file says `roster-z`.
    #[test]
    fn the_roster_is_in_file_order_not_sorted_order() {
        if in_child() {
            file_order_body(&Sandbox::attached());
            return;
        }
        let sb = Sandbox::build("file-order", ROSTER_Z_FIRST);
        sb.arm("cand-a");
        assert!(
            sb.spawn_child("the_roster_is_in_file_order_not_sorted_order"),
            "the sandboxed body failed in the child"
        );
    }

    fn file_order_body(sb: &Sandbox) {
        assert_eq!(run_cmd(&sb.bead, "fb", sb.target), 0);
        assert!(
            sb.critic_wt("roster-z").is_dir(),
            "the first FILE entry that is not the subject reviews it"
        );
        assert!(
            !sb.critic_wt("roster-a").exists(),
            "roster order is sources.toml order, not alphabetical"
        );
    }

    /// Clause 6: a `disabled` arm is never cast as a critic, and it is pinned where the
    /// first eligible arm would otherwise be chosen -- `switched-off` sits FIRST in the
    /// file, so a roster that did not filter by status would cast it.
    #[test]
    fn a_disabled_arm_is_never_cast_as_critic() {
        if in_child() {
            disabled_first_body(&Sandbox::attached());
            return;
        }
        let sb = Sandbox::build("disabled-first", DISABLED_FIRST);
        sb.arm("cand-a");
        assert!(
            sb.spawn_child("a_disabled_arm_is_never_cast_as_critic"),
            "the sandboxed body failed in the child"
        );
    }

    fn disabled_first_body(sb: &Sandbox) {
        assert_eq!(run_cmd(&sb.bead, "fb", sb.target), 0);
        assert!(
            sb.critic_wt("eligible-e").is_dir(),
            "the first ELIGIBLE arm reviews the subject"
        );
        assert!(
            !sb.critic_wt("switched-off").exists(),
            "a disabled arm is never cast"
        );
    }

    /// Clause 9: an unrecognised `status` is not roster-eligible. `hibernating` is a
    /// status this specification does not name; treating it as castable would put this
    /// implementation of the rule in disagreement with `fb eligible`, which refuses it.
    #[test]
    fn an_unrecognised_status_is_not_roster_eligible() {
        if in_child() {
            unknown_status_body(&Sandbox::attached());
            return;
        }
        let sb = Sandbox::build("unknown-status", UNKNOWN_STATUS_FIRST);
        sb.arm("cand-a");
        assert!(
            sb.spawn_child("an_unrecognised_status_is_not_roster_eligible"),
            "the sandboxed body failed in the child"
        );
    }

    fn unknown_status_body(sb: &Sandbox) {
        assert_eq!(run_cmd(&sb.bead, "fb", sb.target), 0);
        assert!(sb.critic_wt("eligible-e").is_dir());
        assert!(
            !sb.critic_wt("hibernate-h").exists(),
            "a status the spec does not name is excluded like disabled"
        );
    }

    /// The roster statuses accepted are exactly `verified` and `untested` -- so
    /// `untested`, the second accepted value, must actually be accepted.
    #[test]
    fn an_untested_arm_is_roster_eligible() {
        if in_child() {
            untested_body(&Sandbox::attached());
            return;
        }
        let sb = Sandbox::build("untested-only", UNTESTED_ONLY);
        sb.arm("cand-a");
        assert!(
            sb.spawn_child("an_untested_arm_is_roster_eligible"),
            "the sandboxed body failed in the child"
        );
    }

    fn untested_body(sb: &Sandbox) {
        assert_eq!(run_cmd(&sb.bead, "fb", sb.target), 0);
        assert!(
            sb.critic_wt("fresh-f").is_dir(),
            "untested is the second accepted status"
        );
    }

    /// Clause 7: the critic never writes into the subject's worktree -- that worktree is
    /// the artefact being measured, and it comes out of the run exactly as it went in.
    #[test]
    fn the_critic_never_writes_into_the_subject_worktree() {
        if in_child() {
            clean_subject_body(&Sandbox::attached());
            return;
        }
        let sb = Sandbox::build("clean-subject", ROSTER_B);
        sb.arm("cand-a");
        assert!(
            sb.spawn_child("the_critic_never_writes_into_the_subject_worktree"),
            "the sandboxed body failed in the child"
        );
    }

    fn clean_subject_body(sb: &Sandbox) {
        let subject = sb.subject_wt("cand-a");
        let deliverable = subject.join(sb.target);
        assert_eq!(run_cmd(&sb.bead, "fb", sb.target), 0);
        assert_eq!(
            entries(&subject),
            vec![sb.target.to_string()],
            "the subject worktree holds exactly what the candidate wrote"
        );
        assert_eq!(
            fs::read_to_string(&deliverable).unwrap_or_default(),
            "pub fn delivered() {}\n",
            "the deliverable itself is unmodified"
        );
    }

    /// Clause 8: two critics reviewing the same subject get two places to write, not one
    /// overwriting the other. The critic's working directory is pinned at
    /// `<worktrees>/<bead>--critic--<arm>`, so two critics of one subject -- one per
    /// run, since one field casts one critic per subject -- leave two distinct paths
    /// standing, and the report filed under `logs/critiques/<bead>/` is named
    /// `<critic>.on.<subject>.md` for the same reason.
    #[test]
    fn two_critics_of_one_subject_get_two_places_to_write() {
        if in_child() {
            two_critics_body(&Sandbox::attached());
            return;
        }
        let sb = Sandbox::build("two-critics", ROSTER_B);
        sb.arm("cand-a");
        assert!(
            sb.spawn_child("two_critics_of_one_subject_get_two_places_to_write"),
            "the sandboxed body failed in the child"
        );
    }

    fn two_critics_body(sb: &Sandbox) {
        assert_eq!(run_cmd(&sb.bead, "fb", sb.target), 0);
        assert!(sb.critic_wt("roster-b").is_dir());
        sb.set_sources(
            r#"
[source.roster-c]
status = "verified"
"#,
        );
        assert_eq!(run_cmd(&sb.bead, "fb", sb.target), 0);
        assert!(sb.critic_wt("roster-c").is_dir());
        assert!(
            sb.critic_wt("roster-b").is_dir(),
            "the second critic must not take the first critic's place"
        );
    }

    /// A roster that cannot be read is a failure, not an empty roster -- and both spell
    /// themselves 1, never 0 and never 4.
    #[test]
    fn an_unreadable_roster_is_a_failure_not_an_empty_roster() {
        if in_child() {
            unreadable_roster_body(&Sandbox::attached());
            return;
        }
        let sb = Sandbox::build("bad-toml", "this is = = not toml [[");
        sb.arm("cand-a");
        assert!(
            sb.spawn_child("an_unreadable_roster_is_a_failure_not_an_empty_roster"),
            "the sandboxed body failed in the child"
        );
    }

    fn unreadable_roster_body(sb: &Sandbox) {
        let code = run_cmd(&sb.bead, "fb", sb.target);
        assert_eq!(code, 1);
        assert_ne!(code, 4);
        assert_ne!(code, 0);
    }
}
