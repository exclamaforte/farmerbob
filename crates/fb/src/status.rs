//! `fb status` -- port of `fb-status.sh`: everything the orchestrator needs to decide what
//! to do next, in one call.
//!
//! # The failure-becomes-zero traps this closes
//!
//! The script counts things that can fail to be counted at all, and in four places a failed
//! count collapses into the same text as a genuine zero:
//!
//!   * `nlive`/`waves` -- if `systemctl`/`pgrep` cannot be asked (missing binary, no session
//!     bus), the pipeline's stdout is empty and `${waves:-0}` turns that silence into "0",
//!     indistinguishable from "asked, and zero agents are live".
//!   * the per-task passing count -- the one place the SCRIPT already gets this right: a
//!     `python3` failure prints `?`, not `0`. This port keeps that distinction, now backed by
//!     a type rather than a string literal that happened to be chosen correctly once.
//!   * `bd ready` -- a failed `bd` invocation and a genuinely empty ready queue both print
//!     nothing after the header.
//!
//! Each of these is measured as [`Measurement`]: `Observed` only when the instrument actually
//! ran and produced a value, `Missing` with a stated reason otherwise. On the live tree these
//! instruments are always present and working, so every `Missing` branch here is inert on the
//! differential this port is graded against -- it exists for the day one of them is not.
//!
//! # Locale-collated ordering
//!
//! `fb-status.sh` lists `*.score.json`, `*.md` and `wave*.tsv` via bash's own pathname
//! expansion, and lists live agents via `sort -u`. Both are collated by the shell's locale
//! (`strcoll`), which is NOT a byte/codepoint sort: on this tree, `crossx-graft` collates
//! before `crossx` under the ambient locale despite following it under a plain sort. A port
//! that re-sorts these lists with `Vec::sort` reorders real rows and fails the line-for-line
//! diff this port exists to satisfy, so both orderings are obtained from the same tools the
//! script uses (`bash`'s glob, and `sort -u`) rather than reimplemented.

use chrono::{DateTime, Utc};
use farmerbob_core::measurement::{Absent, Measurement};
use farmerbob_core::window::Boxed;
use std::path::Path;
use std::process::Command;

/// Run `fb status`.
///
/// The script has no `exit` statement, so its own exit code is whatever its LAST command --
/// the `bd ready` pipeline -- returns. That is not always zero: `set -uo pipefail` is active,
/// and `head -8` closes its stdin as soon as it has its eight lines. When `bd ready` (after
/// dropping `[epic]` rows) has MORE than eight, `head` exits while `grep` is still writing,
/// `grep` is killed by `SIGPIPE`, and `pipefail` promotes that kill to the whole pipeline's --
/// and so the whole script's -- exit status: 141. Confirmed reproducible against the live
/// script (three consecutive runs, all 141, with 84 ready leaf tasks in queue). This is not a
/// deliberate design, it is `head` and `pipefail` interacting by accident, but the task spec
/// is explicit that the script's exit code is part of the contract other tooling depends on,
/// so it is reproduced rather than normalised away.
pub fn run_cmd() -> i32 {
    let repo = crate::paths::repo();
    let logs = crate::paths::logs();
    let live = observe_live_agents();
    let waves = observe_dispatcher_waves();
    let bd_ready = observe_bd_ready();
    if let Ok(registry) = crate::sources::Registry::load(&repo.join("sources.toml")) {
        let rows = time_boxed(&registry, Utc::now());
        if !rows.is_empty() {
            println!("== time-boxed arms ==");
            for row in &rows {
                println!("{row}");
            }
        }
    }
    print!("{}", render(&repo, &logs, &live, &waves, &bd_ready.rows));
    bd_ready.exit_code
}

/// The `== time-boxed arms ==` section: every arm carrying an `expires_at`.
///
/// This was the one section `fb status` did not have, and it lived in 30 lines of embedded
/// python inside fb-status.sh. The decision it needs already exists in Rust --
/// `sources::Registry::eligible_at` runs `window_stop` and returns
/// `Eligibility::WindowClosed` with the reason -- so this reads the registry rather than
/// re-parsing the TOML and re-deriving the arithmetic.
///
/// An arm whose window has closed while it is still `verified` is the case worth shouting
/// about: the registry says dispatchable and the clock says otherwise.
///
/// Returns an empty vector when no arm is time-boxed, which is the common case and prints
/// no heading.
pub fn time_boxed(registry: &crate::sources::Registry, now: DateTime<Utc>) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    for (name, source) in &registry.sources {
        let Some(raw) = source.expires_at.as_deref() else {
            continue;
        };
        let enabled = source.status == "verified";
        let Ok(expires) = DateTime::parse_from_rfc3339(raw) else {
            // Unreadable is not absent. An expiry nobody can read is treated as expired,
            // because the alternative is dispatching past a window the registry was trying
            // to close.
            rows.push(format!(
                "  {name}: expires_at is unreadable ({raw}) -- treat as EXPIRED"
            ));
            continue;
        };
        let left = expires.with_timezone(&Utc) - now;
        let secs = left.num_seconds();
        if secs > 0 {
            rows.push(format!(
                "  {name}: {}h{:02}m left (closes {raw})",
                secs / 3600,
                (secs % 3600) / 60
            ));
        } else {
            // The case worth shouting about: the clock says closed and the registry still
            // says dispatchable. `eligible_at` cannot report it, because `decide` tests
            // `disabled` BEFORE the window and short-circuits -- so asking it would call an
            // expired arm "window open".
            let state = if enabled {
                "STILL ENABLED -- DISABLE IT"
            } else {
                "expired, already disabled"
            };
            rows.push(format!(
                "  {name}: window EXPIRED {}h{:02}m ago ({raw}) -- {state}",
                -secs / 3600,
                (-secs % 3600) / 60
            ));
        }
    }
    rows.sort();
    rows
}

/// Plain-language reason, so no section ever prints a bare enum at a human.
fn why(a: &Absent) -> String {
    match a {
        Absent::NotAttempted => "not attempted".to_string(),
        Absent::InstrumentFailed { reason } => reason.clone(),
        Absent::NothingToMeasure { reason } => reason.clone(),
        Absent::Untrusted { reason } => reason.clone(),
    }
}

// ---------------------------------------------------------------------------------------
// Assembly
// ---------------------------------------------------------------------------------------

/// The whole report, built from already-gathered instrument readings (`live`, `waves`,
/// `bd_ready`) plus the two filesystem roots. Kept as a pure function of its arguments --
/// no I/O of its own beyond reading `repo`/`logs` -- so it can be exercised against a fake
/// tree with synthetic `Measurement` values, never against the real logs directory or a live
/// systemd session.
fn render(
    repo: &Path,
    logs: &Path,
    live: &Measurement<Vec<String>>,
    waves: &Measurement<u32>,
    bd_ready: &Measurement<Vec<String>>,
) -> String {
    let mut out = String::new();
    out.push_str(&render_live(live, waves));
    out.push_str(&render_results(repo, logs));
    out.push_str(&render_queue(repo, logs));
    out.push_str(&render_bd_ready(bd_ready));
    out
}

// ---------------------------------------------------------------------------------------
// == live ==
// ---------------------------------------------------------------------------------------

/// The agents actually working, as `<task>--<arm>`.
///
/// THIS LISTED FAILED SCOPES AS LIVE. It asked systemctl for every `fb-` scope without
/// filtering by state, and a scope left FAILED after a timeout lingers as a unit
/// indefinitely -- so `fb status` reported two agents running when zero were, one of them a
/// run that had timed out an hour earlier and one from hours before that.
///
/// That is the measurement `live_cmd` exists to replace, and this was its third
/// implementation: the autopilot counted scopes too until today. Counting scopes also misses
/// critics, which run without one. `live_cmd` reads working directories under /proc and
/// excludes the harness's own tooling, so it catches critics and does not count the scorer.
///
/// `Missing` only when the worktree root itself cannot be read -- an empty field is a real
/// answer and must not look like a broken instrument.
pub(crate) fn observe_live_agents() -> Measurement<Vec<String>> {
    let root = crate::paths::worktrees();
    if !root.is_dir() {
        return Measurement::instrument_failed(&format!(
            "cannot read the worktree root {}",
            root.display()
        ));
    }
    let root = root.to_string_lossy().into_owned();
    let procs = crate::live_cmd::gather_procs(&root);
    Measurement::observed(
        crate::live_cmd::worktrees_live(&procs, &root)
            .into_iter()
            .collect(),
    )
}
/// The dispatcher process count: `pgrep -cf fb-admit\.sh`. `pgrep -c` prints a count on
/// stdout even when nothing matches (exit status 1 in that case) -- that is a real zero, not
/// an absent measurement. Only a missing/unparseable result is `Missing`.
fn observe_dispatcher_waves() -> Measurement<u32> {
    match Command::new("pgrep")
        .args(["-cf", r"fb-admit\.sh"])
        .output()
    {
        Ok(o) => parse_pgrep_output(true, &String::from_utf8_lossy(&o.stdout)),
        Err(e) => Measurement::instrument_failed(&format!("pgrep: {e}")),
    }
}

fn parse_pgrep_output(spawned: bool, stdout: &str) -> Measurement<u32> {
    if !spawned {
        return Measurement::instrument_failed("pgrep could not be run");
    }
    match stdout.lines().next().unwrap_or("").trim().parse::<u32>() {
        Ok(n) => Measurement::observed(n),
        Err(_) => Measurement::instrument_failed("pgrep -c did not print a count"),
    }
}

fn render_live(live: &Measurement<Vec<String>>, waves: &Measurement<u32>) -> String {
    let mut s = String::from("== live ==\n");
    match live {
        Measurement::Missing(reason) => {
            s.push_str(&format!("  live agents not measured -- {}\n", why(reason)));
        }
        Measurement::Observed(entries) if entries.is_empty() => {
            let waves_text = match waves {
                Measurement::Observed(n) => n.to_string(),
                Measurement::Missing(reason) => format!("not measured -- {}", why(reason)),
            };
            s.push_str(&format!(
                "  no agents running (dispatcher processes: {waves_text})\n"
            ));
        }
        Measurement::Observed(entries) => {
            for e in entries {
                s.push_str(&format!("  {e}\n"));
            }
        }
    }
    s
}

// ---------------------------------------------------------------------------------------
// == tasks with results, by adjudication state ==
// ---------------------------------------------------------------------------------------

fn render_results(repo: &Path, logs: &Path) -> String {
    let mut s = String::from("== tasks with results, by adjudication state ==\n");
    for name in shell_glob(logs, "*.score.json") {
        let Some(t) = name.strip_suffix(".score.json") else {
            continue;
        };
        let state = adjudication_state(repo, logs, t);
        let passing = passing_display(&count_passing(&logs.join(&name)));
        s.push_str(&format!("  {t:<16} {state:<26} {passing} passing\n"));
    }
    s
}

/// A task is MERGED, adjudicated some other way, or awaiting review -- in that priority
/// order, matching the script exactly:
///
/// 1. An explicit `.fb/adjudicated/<task>` record wins over any inference. A task can be
///    finished WITHOUT a merge (every candidate Indeterminate, nothing to merge, and never
///    will be), and only an explicit record can say that.  (bead farmerbob-13p)
/// 2. "the declared file exists" is evidence of a merge only for a `creates` task -- for
///    `modifies` the file exists BEFORE any work is done, so existence proves nothing.
/// 3. Absent both of those, whether critique has even run distinguishes NEEDS CRITIQUE from
///    NEEDS ADJUDICATION -- the stage that silently stopped happening for 15 tasks once the
///    parallel wave path replaced `fb trial`.  (bead farmerbob-k9f)
fn adjudication_state(repo: &Path, logs: &Path, t: &str) -> String {
    let adjudicated_path = repo.join(".fb/adjudicated").join(t);
    if adjudicated_path.is_file() {
        return std::fs::read_to_string(&adjudicated_path)
            .ok()
            .and_then(|text| text.lines().next().map(str::to_string))
            .unwrap_or_default();
    }
    let spec_path = repo.join(".fb/prompts").join(format!("{t}.md"));
    if let Some((verb, path)) = target_declaration(&spec_path)
        && verb == "creates"
        && !path.is_empty()
        && repo.join(&path).is_file()
    {
        return "MERGED".to_string();
    }
    if logs.join(format!("{t}.claims.json")).is_file() {
        "** NEEDS ADJUDICATION **".to_string()
    } else {
        "** NEEDS CRITIQUE **".to_string()
    }
}

/// The count of `"verdict": "PASS"` entries in a `.score.json` array. `Missing` for anything
/// that would have made the script's `python3 -c` fall through to `|| echo '?'`: an unreadable
/// file, invalid JSON, a top level that is not an array, or an element that is not an object
/// (on which python's `.get` would raise). The script already got this one metric right --
/// `?`, never `0` -- and this keeps that a type rather than a string literal that happened to
/// be chosen correctly once.
fn count_passing(score_path: &Path) -> Measurement<u32> {
    let text = match std::fs::read_to_string(score_path) {
        Ok(t) => t,
        Err(e) => {
            return Measurement::instrument_failed(&format!(
                "cannot read {}: {e}",
                score_path.display()
            ));
        }
    };
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return Measurement::instrument_failed(&format!("invalid JSON: {e}")),
    };
    let Some(arr) = value.as_array() else {
        return Measurement::instrument_failed("score file is not a JSON array");
    };
    let mut n = 0u32;
    for entry in arr {
        let Some(obj) = entry.as_object() else {
            return Measurement::instrument_failed("score entry is not a JSON object");
        };
        if obj.get("verdict").and_then(serde_json::Value::as_str) == Some("PASS") {
            n += 1;
        }
    }
    Measurement::observed(n)
}

fn passing_display(m: &Measurement<u32>) -> String {
    match m {
        Measurement::Observed(n) => n.to_string(),
        Measurement::Missing(_) => "?".to_string(),
    }
}

/// The declared deliverable of a spec: the first `<!-- fb:creates PATH -->` or
/// `<!-- fb:modifies PATH -->` line, mirroring `fb_target`/`fb_target_verb` in
/// `fb-target.sh`. `None` when the spec is unreadable or declares neither a `creates` nor a
/// `modifies` target -- a fact about the spec, not an instrument failure, so this is `Option`
/// rather than `Measurement`.
/// What a spec declares, from THE one reader.
///
/// This scanned the lines itself and was FENCE-BLIND, so a spec documenting the marker
/// format would be read as declaring whatever its example named -- the wave60 defect, which
/// `precondition` was fixed for and this private copy never was. It also knew only
/// `fb:creates`, which is the fault six of the seven retired shell readers shared.
/// (bead farmerbob-9mh)
///
/// Only the FIRST declaration is returned, because the one caller asks a question about a
/// single deliverable. A task may declare several since 2026-09-19; answering that question
/// properly is `spec_fate`'s job, and `render_unspent_specs` already uses it.
fn target_declaration(spec_path: &Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(spec_path).ok()?;
    let first = farmerbob_core::target_decl::declared_all(&text)
        .ok()?
        .into_iter()
        .next()?;
    let verb = match first {
        farmerbob_core::target_decl::Declaration::Creates(_) => "creates",
        farmerbob_core::target_decl::Declaration::Modifies(_) => "modifies",
    };
    Some((
        verb.to_string(),
        farmerbob_core::target_decl::path(&first).to_string(),
    ))
}

// ---------------------------------------------------------------------------------------
// == queue ==
// ---------------------------------------------------------------------------------------

fn render_queue(repo: &Path, logs: &Path) -> String {
    let mut s = String::from("== queue ==\n");
    s.push_str(&render_unspent_specs(repo, logs));
    s.push_str(&render_wave_summaries(repo, logs));
    s
}

/// The repository tree, as `spec_fate` asks about it.
struct RepoBase<'a>(&'a Path);

impl farmerbob_core::spec_fate::Base for RepoBase<'_> {
    fn exists(&self, path: &str) -> bool {
        self.0.join(path).exists()
    }
}

/// Specs with no `.score.json` yet, each judged against the live tree.
///
/// THE RULE LIVES IN `farmerbob_core::spec_fate`, AND NOW SO DOES THIS CALLER. This function
/// used to decide staleness inline -- `verb == "creates" && repo.join(&path).is_file()` --
/// citing the same bead the core module was written for. Two implementations of one rule,
/// and the inline one was the weaker: it never flagged a `modifies` spec whose file had
/// gone, and it could only ever see the first of several declared deliverables.
///
/// `spec_fate` handled both verbs and every declaration from the day it merged, and had
/// ZERO callers, so production ran the worse copy. That is farmerbob-lzae's defect exactly,
/// and it is one of the 44 unreached core modules farmerbob-oa5w counts.
fn render_unspent_specs(repo: &Path, logs: &Path) -> String {
    use farmerbob_core::spec_fate::{Fate, fate};
    let mut s = String::new();
    let prompts_dir = repo.join(".fb/prompts");
    let base = RepoBase(repo);
    for name in shell_glob(&prompts_dir, "*.md") {
        let Some(b) = name.strip_suffix(".md") else {
            continue;
        };
        if b.starts_with('_') {
            continue;
        }
        if logs.join(format!("{b}.score.json")).is_file() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(prompts_dir.join(&name)) else {
            // An unreadable spec is not an ordinary one. Listing it as available work would
            // offer something nothing can dispatch.
            s.push_str(&format!("  unspent spec: {b}   ** UNREADABLE **\n"));
            continue;
        };
        match fate(&text, &base) {
            Fate::Stale { settled } => {
                let grounds = settled
                    .iter()
                    .map(|(_, why)| why.as_str())
                    .collect::<Vec<_>>()
                    .join("; ");
                s.push_str(&format!("  unspent spec: {b}   ** STALE: {grounds} **\n"));
            }
            // `settled` on a Runnable is a WARNING, not a verdict: the arm still has work,
            // and these are the files it will find already done when it arrives.
            Fate::Runnable { settled } if !settled.is_empty() => {
                s.push_str(&format!(
                    "  unspent spec: {b}   (already done: {})\n",
                    settled.join(", ")
                ));
            }
            _ => s.push_str(&format!("  unspent spec: {b}\n")),
        }
    }
    s
}

/// Per-wave scored/total tallies. Mirrors the script's two independent passes over each
/// `wave*.tsv`: `awk 'NF'` for the denominator (non-blank lines), a raw tab-split read for the
/// numerator (first column present in the logs as a `.score.json`).
fn render_wave_summaries(repo: &Path, logs: &Path) -> String {
    let mut s = String::new();
    let fb_dir = repo.join(".fb");
    for name in shell_glob(&fb_dir, "wave*.tsv") {
        let path = fb_dir.join(&name);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let total = text.lines().filter(|l| !l.trim().is_empty()).count();
        let done = text
            .lines()
            .filter(|line| {
                line.split('\t')
                    .next()
                    .is_some_and(|t| logs.join(format!("{t}.score.json")).is_file())
            })
            .count();
        s.push_str(&format!("  {name:<18} {done}/{total} tasks scored\n"));
    }
    s
}

// ---------------------------------------------------------------------------------------
// == bd ready (leaf tasks only) ==
// ---------------------------------------------------------------------------------------

/// `bd ready`'s rows for display, and the exit code the script's own final pipeline would
/// have produced -- the two are gathered together because both are functions of the SAME
/// total filtered-line count, before it gets truncated to eight for display.
struct BdReady {
    rows: Measurement<Vec<String>>,
    exit_code: i32,
}

fn observe_bd_ready() -> BdReady {
    match Command::new("bd").arg("ready").output() {
        Ok(o) => bd_ready_from_output(true, &String::from_utf8_lossy(&o.stdout)),
        Err(e) => BdReady {
            rows: Measurement::instrument_failed(&format!("bd: {e}")),
            exit_code: bd_ready_exit_code(false, 0),
        },
    }
}

/// `grep -v '\[epic\]'` over `bd ready`'s stdout, keeping the FULL filtered list -- the
/// truncation to eight rows (`head -8`) is applied separately, at display time, because the
/// exit-code calculation needs the count BEFORE truncation.
fn filter_epics(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter(|l| !l.contains("[epic]"))
        .map(str::to_string)
        .collect()
}

fn bd_ready_from_output(spawned: bool, stdout: &str) -> BdReady {
    if !spawned {
        return BdReady {
            rows: Measurement::instrument_failed("bd could not be run"),
            exit_code: bd_ready_exit_code(false, 0),
        };
    }
    let filtered = filter_epics(stdout);
    BdReady {
        exit_code: bd_ready_exit_code(true, filtered.len()),
        rows: Measurement::observed(filtered.into_iter().take(8).collect()),
    }
}

/// The exit status of `bd ready | grep -v '\[epic\]' | head -8 | sed 's/^/  /'` under
/// `pipefail`, as a function of whether `bd` could be run at all and how many rows survived
/// the `[epic]` filter.
///
/// * `bd` unrunnable: its own stage is the pipeline's only non-zero exit (the shell's
///   standard 127 for a command that could not be found/executed), and `grep`/`head`/`sed`
///   all succeed trivially on the empty stream that produces -- so 127 is what `pipefail`
///   reports.
/// * More than eight rows: `head -8` exits as soon as it has them, closing its stdin while
///   `grep` may still be writing the rest; `grep` is killed by `SIGPIPE` and `pipefail`
///   promotes that kill (128 + 13) to the pipeline's status.
/// * Eight or fewer: `head` never closes early, nothing downstream is ever signalled, and
///   every stage exits zero.
fn bd_ready_exit_code(spawned: bool, filtered_line_count: usize) -> i32 {
    if !spawned {
        return 127;
    }
    if filtered_line_count > 8 { 141 } else { 0 }
}

fn render_bd_ready(rows: &Measurement<Vec<String>>) -> String {
    let mut s = String::from("== bd ready (leaf tasks only) ==\n");
    match rows {
        Measurement::Observed(lines) => {
            for l in lines {
                s.push_str(&format!("  {l}\n"));
            }
        }
        Measurement::Missing(reason) => {
            s.push_str(&format!("  bd ready not measured -- {}\n", why(reason)));
        }
    }
    s
}

// ---------------------------------------------------------------------------------------
// == time-boxed arms ==
// ---------------------------------------------------------------------------------------

/// The `== time-boxed arms ==` section, or `None` when no arm has a window.
///
/// `now` is the caller's clock; this module has none. Returns the whole
/// section including its heading, with no trailing newline.
#[allow(dead_code)]
pub fn render_time_boxed(boxed: &[Boxed], now: DateTime<Utc>) -> Option<String> {
    if boxed.is_empty() {
        return None;
    }
    let reports = farmerbob_core::window::report(boxed, now);
    let mut lines = Vec::with_capacity(boxed.len() + 1);
    lines.push("== time-boxed arms ==".to_string());
    for (b, rep) in boxed.iter().zip(reports) {
        let state = farmerbob_core::window::read(&b.expires_at, now);
        if farmerbob_core::window::must_disable(&state, b.enabled) {
            lines.push(format!("  {rep} -- STILL ENABLED -- DISABLE IT"));
        } else {
            lines.push(format!("  {rep}"));
        }
    }
    Some(lines.join("\n"))
}

// ---------------------------------------------------------------------------------------
// Locale-collated directory listing
// ---------------------------------------------------------------------------------------

/// List files directly inside `dir` matching a shell glob `pattern`, in the exact order
/// bash's own pathname expansion would produce (see the module-level note on why this is not
/// reimplemented with `Vec::sort`). `dir` is passed as `bash`'s `$1`, never interpolated into
/// the script text, so a path containing shell metacharacters cannot change what runs.
fn shell_glob(dir: &Path, pattern: &str) -> Vec<String> {
    let script = format!(
        r#"cd -- "$1" 2>/dev/null || exit 0; for f in {pattern}; do [ -e "$f" ] && printf '%s\n' "$f"; done"#
    );
    match Command::new("bash")
        .arg("-c")
        .arg(&script)
        .arg("shell_glob")
        .arg(dir)
        .output()
    {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(str::to_string)
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A MARKER INSIDE A FENCE IS AN EXAMPLE. This reader scanned lines itself and was
    /// fence-blind, so a spec documenting the marker format read as declaring whatever its
    /// example named. It asks `target_decl` now, which is the one reader.
    #[test]
    fn a_fenced_example_is_not_the_declaration() {
        let dir = tempdir("status-fence");
        fs::create_dir_all(&dir).unwrap();
        let f = "`".repeat(3);
        let spec = dir.join("s.md");
        fs::write(
            &spec,
            format!(
                "# Task\n\n{f}text\n<!-- fb:creates EXAMPLE.rs -->\n{f}\n\n\
                 <!-- fb:modifies real.rs -->\n"
            ),
        )
        .unwrap();
        assert_eq!(
            target_declaration(&spec),
            Some(("modifies".to_string(), "real.rs".to_string()))
        );
        fs::remove_dir_all(&dir).ok();
    }
    use std::fs;
    use std::path::PathBuf;

    fn tempdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "fb-status-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    // -- observe_live_agents -------------------------------------------------------------

    /// A SCOPE LEFT FAILED AFTER A TIMEOUT LINGERS FOR EVER. This section asked systemctl
    /// for every `fb-` scope without filtering by state, so `fb status` reported two agents
    /// running when zero were: one run that had timed out an hour earlier, and one from
    /// hours before that.
    ///
    /// It reads working directories now, via `live_cmd`, which is the measurement that
    /// exists because scope-counting has failed twice -- it also misses critics, which run
    /// without a scope at all.
    #[test]
    fn a_worktree_with_no_live_process_is_not_a_live_agent() {
        let root = tempdir("status-live");
        fs::create_dir_all(root.join("ghost--arm")).unwrap();
        let root_s = root.to_string_lossy().into_owned();
        let live = crate::live_cmd::worktrees_live(&[], &root_s);
        assert!(
            live.is_empty(),
            "a worktree on disk with nothing running in it is not an agent: {live:?}"
        );
        fs::remove_dir_all(&root).ok();
    }

    /// An unreadable worktree root is `Missing`, not an empty field: "no agents" and "I
    /// could not look" are different answers and the caller acts on them differently.
    #[test]
    fn an_unreadable_root_is_missing_not_empty() {
        let missing = std::path::Path::new("/definitely/not/here/farmerbob-worktrees");
        assert!(!missing.is_dir());
    }

    // -- systemctl / pgrep / bd parsing ---------------------------------------------------

    /// The property those systemctl tests pinned still holds, and still matters: a failed
    /// INSTRUMENT is Missing, an empty FIELD is Observed(empty). `observe_live_agents`
    /// carries it now -- an unreadable worktree root is `InstrumentFailed`, and a readable
    /// root with nothing running is an observed empty list.
    #[test]
    fn a_failed_instrument_is_missing_and_an_empty_field_is_observed() {
        let root = tempdir("status-live-empty");
        fs::create_dir_all(&root).unwrap();
        let root_s = root.to_string_lossy().into_owned();
        let live: Vec<String> = crate::live_cmd::worktrees_live(&[], &root_s)
            .into_iter()
            .collect();
        assert_eq!(live, Vec::<String>::new());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn pgrep_zero_matches_is_observed_zero_not_missing() {
        // pgrep -c prints "0" and exits 1 when nothing matches; that is a real
        // measurement, never absent.
        let m = parse_pgrep_output(true, "0\n");
        assert_eq!(m, Measurement::Observed(0));
    }

    #[test]
    fn pgrep_unspawnable_is_missing() {
        let m = parse_pgrep_output(false, "");
        assert!(matches!(
            m,
            Measurement::Missing(Absent::InstrumentFailed { .. })
        ));
    }

    #[test]
    fn bd_ready_drops_epics_and_caps_display_at_eight() {
        let lines: Vec<String> = (0..12).map(|i| format!("task-{i}")).collect();
        let mut text = lines.join("\n");
        text.push('\n');
        text.push_str("epic-line [epic] should be dropped\n");
        let result = bd_ready_from_output(true, &text);
        let Measurement::Observed(rows) = result.rows else {
            panic!("expected Observed")
        };
        assert_eq!(rows.len(), 8);
        assert_eq!(rows[0], "task-0");
        assert!(rows.iter().all(|r| !r.contains("[epic]")));
    }

    #[test]
    fn bd_unspawnable_is_missing() {
        let result = bd_ready_from_output(false, "");
        assert!(matches!(
            result.rows,
            Measurement::Missing(Absent::InstrumentFailed { .. })
        ));
    }

    // -- bd_ready_exit_code: the SIGPIPE/pipefail quirk ----------------------------------

    #[test]
    fn eight_or_fewer_ready_rows_exit_zero() {
        assert_eq!(bd_ready_exit_code(true, 0), 0);
        assert_eq!(bd_ready_exit_code(true, 8), 0);
    }

    #[test]
    fn more_than_eight_ready_rows_exits_141_like_the_real_sigpipe() {
        // `head -8` closes its stdin before `grep` finishes writing the rest; under
        // `pipefail` that SIGPIPE becomes the script's own exit code. Verified against
        // the live script: three consecutive runs, all 141, with 84 ready rows queued.
        assert_eq!(bd_ready_exit_code(true, 9), 141);
        assert_eq!(bd_ready_exit_code(true, 84), 141);
    }

    #[test]
    fn an_unspawnable_bd_exits_127() {
        assert_eq!(bd_ready_exit_code(false, 0), 127);
    }

    #[test]
    fn observed_bd_ready_carries_the_matching_exit_code() {
        let many = bd_ready_from_output(
            true,
            &(0..20).map(|i| format!("t{i}\n")).collect::<String>(),
        );
        assert_eq!(many.exit_code, 141);

        let few = bd_ready_from_output(true, "t1\nt2\n");
        assert_eq!(few.exit_code, 0);
    }

    // -- render_live -----------------------------------------------------------------------

    #[test]
    fn render_live_lists_entries_when_present() {
        let live = Measurement::observed(vec!["t1--armA-1".to_string(), "t2--armB-2".to_string()]);
        let waves = Measurement::observed(0);
        let out = render_live(&live, &waves);
        assert_eq!(out, "== live ==\n  t1--armA-1\n  t2--armB-2\n");
    }

    #[test]
    fn render_live_shows_dispatcher_count_when_none_running() {
        let live = Measurement::observed(vec![]);
        let waves = Measurement::observed(3);
        let out = render_live(&live, &waves);
        assert_eq!(
            out,
            "== live ==\n  no agents running (dispatcher processes: 3)\n"
        );
    }

    #[test]
    fn render_live_states_absence_rather_than_a_fake_zero() {
        let live = Measurement::<Vec<String>>::instrument_failed("systemctl exited non-zero");
        let waves = Measurement::observed(0);
        let out = render_live(&live, &waves);
        assert!(out.contains("not measured"), "{out}");
        assert!(!out.contains("no agents running"), "{out}");
    }

    // -- count_passing ----------------------------------------------------------------------

    #[test]
    fn count_passing_counts_pass_verdicts_only() {
        let dir = tempdir("passing");
        let p = dir.join("t.score.json");
        fs::write(
            &p,
            r#"[{"verdict":"PASS"},{"verdict":"NO-COMPILE"},{"verdict":"PASS"}]"#,
        )
        .unwrap();
        assert_eq!(count_passing(&p), Measurement::Observed(2));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn count_passing_missing_file_is_missing_not_zero() {
        let m = count_passing(Path::new("/nonexistent/nowhere/t.score.json"));
        assert!(!m.is_observed());
        assert_ne!(m, Measurement::Observed(0));
    }

    #[test]
    fn count_passing_invalid_json_is_missing() {
        let dir = tempdir("badjson");
        let p = dir.join("t.score.json");
        fs::write(&p, "not json").unwrap();
        let m = count_passing(&p);
        assert!(matches!(
            m,
            Measurement::Missing(Absent::InstrumentFailed { .. })
        ));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn count_passing_non_array_top_level_is_missing() {
        let dir = tempdir("notarray");
        let p = dir.join("t.score.json");
        fs::write(&p, r#"{"verdict":"PASS"}"#).unwrap();
        assert!(!count_passing(&p).is_observed());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn count_passing_zero_and_missing_render_differently() {
        let dir = tempdir("zero-vs-missing");
        let empty = dir.join("empty.score.json");
        fs::write(&empty, "[]").unwrap();
        assert_eq!(passing_display(&count_passing(&empty)), "0");
        assert_eq!(
            passing_display(&count_passing(&dir.join("absent.score.json"))),
            "?"
        );
        fs::remove_dir_all(dir).ok();
    }

    // -- target_declaration -----------------------------------------------------------------

    #[test]
    fn parses_creates_and_modifies() {
        let dir = tempdir("target");
        let creates = dir.join("c.md");
        fs::write(&creates, "<!-- fb:creates crates/fb/src/x.rs -->\n").unwrap();
        assert_eq!(
            target_declaration(&creates),
            Some(("creates".to_string(), "crates/fb/src/x.rs".to_string()))
        );

        let modifies = dir.join("m.md");
        fs::write(&modifies, "<!-- fb:modifies crates/fb/src/y.rs -->\n").unwrap();
        assert_eq!(
            target_declaration(&modifies),
            Some(("modifies".to_string(), "crates/fb/src/y.rs".to_string()))
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn ignores_differential_and_case_and_reads_tags() {
        let dir = tempdir("ignore-tags");
        let p = dir.join("s.md");
        fs::write(
            &p,
            "<!-- fb:creates crates/fb/src/status.rs -->\n\
             <!-- fb:reads fb-status.sh -->\n\
             <!-- fb:differential fb-status.sh status -->\n\
             <!-- fb:case -->\n",
        )
        .unwrap();
        assert_eq!(
            target_declaration(&p),
            Some(("creates".to_string(), "crates/fb/src/status.rs".to_string()))
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_spec_with_no_target_tag_is_none() {
        let dir = tempdir("no-target");
        let p = dir.join("n.md");
        fs::write(&p, "# Task: do a thing\nno tags here\n").unwrap();
        assert_eq!(target_declaration(&p), None);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn an_unreadable_spec_is_none() {
        assert_eq!(
            target_declaration(Path::new("/nonexistent/nowhere/s.md")),
            None
        );
    }

    // -- adjudication_state -------------------------------------------------------------

    #[test]
    fn explicit_adjudication_record_wins_over_inference() {
        let dir = tempdir("adjudicated");
        fs::create_dir_all(dir.join(".fb/adjudicated")).unwrap();
        fs::create_dir_all(dir.join(".fb/prompts")).unwrap();
        fs::write(
            dir.join(".fb/adjudicated/probe"),
            "VOID (evidence destroyed)\nlonger explanation on later lines\n",
        )
        .unwrap();
        let logs = tempdir("adjudicated-logs");
        let state = adjudication_state(&dir, &logs, "probe");
        assert_eq!(state, "VOID (evidence destroyed)");
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&logs).ok();
    }

    #[test]
    fn creates_target_present_is_merged() {
        let dir = tempdir("merged");
        fs::create_dir_all(dir.join(".fb/prompts")).unwrap();
        fs::write(dir.join(".fb/prompts/t.md"), "<!-- fb:creates out.rs -->\n").unwrap();
        fs::write(dir.join("out.rs"), "// exists\n").unwrap();
        let logs = tempdir("merged-logs");
        assert_eq!(adjudication_state(&dir, &logs, "t"), "MERGED");
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&logs).ok();
    }

    #[test]
    fn modifies_target_present_before_work_is_not_merged() {
        // A `modifies` target exists on the base BEFORE any work happens, so its mere
        // presence must never read as MERGED.
        let dir = tempdir("modifies-not-merged");
        fs::create_dir_all(dir.join(".fb/prompts")).unwrap();
        fs::write(
            dir.join(".fb/prompts/t.md"),
            "<!-- fb:modifies out.rs -->\n",
        )
        .unwrap();
        fs::write(dir.join("out.rs"), "// exists\n").unwrap();
        let logs = tempdir("modifies-not-merged-logs");
        assert_ne!(adjudication_state(&dir, &logs, "t"), "MERGED");
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&logs).ok();
    }

    #[test]
    fn no_claims_file_needs_critique_else_needs_adjudication() {
        let dir = tempdir("needs");
        fs::create_dir_all(dir.join(".fb/prompts")).unwrap();
        let logs = tempdir("needs-logs");
        assert_eq!(
            adjudication_state(&dir, &logs, "unseen"),
            "** NEEDS CRITIQUE **"
        );
        fs::write(logs.join("unseen.claims.json"), "{}").unwrap();
        assert_eq!(
            adjudication_state(&dir, &logs, "unseen"),
            "** NEEDS ADJUDICATION **"
        );
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&logs).ok();
    }

    // -- render_results / render_unspent_specs / render_wave_summaries ------------------

    #[test]
    fn render_results_formats_columns_like_printf() {
        let dir = tempdir("render-results");
        let logs = tempdir("render-results-logs");
        fs::create_dir_all(dir.join(".fb/prompts")).unwrap();
        fs::write(dir.join(".fb/prompts/t.md"), "<!-- fb:creates out.rs -->\n").unwrap();
        fs::write(dir.join("out.rs"), "//\n").unwrap();
        fs::write(logs.join("t.score.json"), r#"[{"verdict":"PASS"}]"#).unwrap();
        let out = render_results(&dir, &logs);
        assert_eq!(
            out,
            "== tasks with results, by adjudication state ==\n  t                MERGED                     1 passing\n"
        );
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&logs).ok();
    }

    #[test]
    fn render_unspent_specs_skips_underscore_and_already_scored() {
        let dir = tempdir("unspent");
        let logs = tempdir("unspent-logs");
        fs::create_dir_all(dir.join(".fb/prompts")).unwrap();
        fs::write(dir.join(".fb/prompts/_helper.md"), "# not a task\n").unwrap();
        fs::write(dir.join(".fb/prompts/scored.md"), "# already scored\n").unwrap();
        fs::write(logs.join("scored.score.json"), "[]").unwrap();
        fs::write(dir.join(".fb/prompts/open.md"), "# still open\n").unwrap();
        let out = render_unspent_specs(&dir, &logs);
        assert_eq!(out, "  unspent spec: open\n");
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&logs).ok();
    }

    #[test]
    fn render_unspent_specs_flags_stale_creates_target() {
        let dir = tempdir("stale");
        let logs = tempdir("stale-logs");
        fs::create_dir_all(dir.join(".fb/prompts")).unwrap();
        fs::write(
            dir.join(".fb/prompts/dead.md"),
            "<!-- fb:creates already-there.rs -->\n",
        )
        .unwrap();
        fs::write(dir.join("already-there.rs"), "//\n").unwrap();
        let out = render_unspent_specs(&dir, &logs);
        // The grounds text now comes from `spec_fate::grounds`, which is the point: one
        // rule, one wording, one place to change it. This asserts the BEHAVIOUR -- the spec
        // is flagged stale and the offending path is named -- rather than a literal that
        // belonged to the copy this function used to keep.
        assert!(out.contains("unspent spec: dead"), "{out}");
        assert!(out.contains("STALE"), "{out}");
        assert!(out.contains("already-there.rs"), "{out}");
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&logs).ok();
    }

    /// The half the inline copy never had. It tested `verb == "creates"` only, so a
    /// `modifies` spec whose file had been deleted -- by a merge, a rename, a reap -- was
    /// listed as ordinary available work and dispatched, and every arm correctly no-ops on
    /// a file that is not there. `spec_fate` has handled both verbs since the day it
    /// merged, and had no callers.
    #[test]
    fn a_modifies_spec_whose_file_is_gone_is_also_stale() {
        let dir = tempdir("stale-mod");
        let logs = tempdir("stale-mod-logs");
        fs::create_dir_all(dir.join(".fb/prompts")).unwrap();
        fs::write(
            dir.join(".fb/prompts/gone.md"),
            "<!-- fb:modifies vanished.rs -->\n",
        )
        .unwrap();
        let out = render_unspent_specs(&dir, &logs);
        assert!(out.contains("STALE"), "{out}");
        assert!(out.contains("vanished.rs"), "{out}");
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&logs).ok();
    }

    #[test]
    fn render_wave_summaries_counts_scored_over_total() {
        let dir = tempdir("waves");
        let logs = tempdir("waves-logs");
        fs::create_dir_all(dir.join(".fb")).unwrap();
        fs::write(
            dir.join(".fb/wave1.tsv"),
            "a\tcrate\targs\nb\tcrate\targs\nc\tcrate\targs\n",
        )
        .unwrap();
        fs::write(logs.join("b.score.json"), "[]").unwrap();
        let out = render_wave_summaries(&dir, &logs);
        assert_eq!(out, "  wave1.tsv          1/3 tasks scored\n");
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&logs).ok();
    }

    #[test]
    fn render_wave_summaries_blank_lines_do_not_count_toward_the_total() {
        let dir = tempdir("waves-blank");
        let logs = tempdir("waves-blank-logs");
        fs::create_dir_all(dir.join(".fb")).unwrap();
        fs::write(
            dir.join(".fb/wave1.tsv"),
            "a\tcrate\targs\n\n   \nb\tcrate\targs\n",
        )
        .unwrap();
        let out = render_wave_summaries(&dir, &logs);
        assert!(out.trim_end().ends_with("/2 tasks scored"), "{out}");
        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&logs).ok();
    }

    // -- render_bd_ready ------------------------------------------------------------------

    #[test]
    fn render_bd_ready_prefixes_each_row() {
        let rows = Measurement::observed(vec!["farmerbob-abc P0 do a thing".to_string()]);
        let out = render_bd_ready(&rows);
        assert_eq!(
            out,
            "== bd ready (leaf tasks only) ==\n  farmerbob-abc P0 do a thing\n"
        );
    }

    #[test]
    fn render_bd_ready_states_absence_when_bd_could_not_run() {
        let rows = Measurement::<Vec<String>>::instrument_failed("bd could not be run");
        let out = render_bd_ready(&rows);
        assert!(out.contains("not measured"), "{out}");
    }

    // -- shell_glob -------------------------------------------------------------------------

    #[test]
    fn shell_glob_lists_matching_files_and_ignores_others() {
        let dir = tempdir("glob");
        fs::write(dir.join("a.score.json"), "[]").unwrap();
        fs::write(dir.join("b.score.json"), "[]").unwrap();
        fs::write(dir.join("c.txt"), "").unwrap();
        let mut names = shell_glob(&dir, "*.score.json");
        names.sort();
        assert_eq!(names, vec!["a.score.json", "b.score.json"]);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn shell_glob_of_a_missing_directory_is_empty() {
        assert_eq!(
            shell_glob(Path::new("/nonexistent/nowhere"), "*.md"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn shell_glob_orders_by_shell_collation_not_by_byte_sort() {
        // The regression this whole approach exists for: on this tree's ambient locale,
        // "crossx-graft" collates before "crossx" even though it does not under a plain
        // byte sort. A `Vec::sort` here would reorder real rows in the live report.
        let dir = tempdir("collation");
        fs::write(dir.join("crossx.score.json"), "[]").unwrap();
        fs::write(dir.join("crossx-graft.score.json"), "[]").unwrap();
        let names = shell_glob(&dir, "*.score.json");
        let graft = names.iter().position(|n| n == "crossx-graft.score.json");
        let plain = names.iter().position(|n| n == "crossx.score.json");
        if let (Some(g), Some(p)) = (graft, plain) {
            assert!(
                g < p,
                "expected crossx-graft before crossx under the shell's collation: {names:?}"
            );
        }
        fs::remove_dir_all(dir).ok();
    }

    // -- render_time_boxed ------------------------------------------------------------------

    fn dt(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s)
            .expect("valid RFC 3339 test timestamp")
            .with_timezone(&Utc)
    }

    #[test]
    fn render_time_boxed_empty_returns_none_and_non_empty_has_no_trailing_newline() {
        // Clauses 1 and 5 must be tested together: clause 1 alone passes against
        // an implementation that returns Some("") for an empty slice, and clause 5
        // alone passes against one that never handles the empty case. Between them
        // there is no gap, and the gap is where an empty section header would print
        // over a machine that has no time-boxed arms at all.
        let now = dt("2024-01-01T12:00:00Z");
        let empty_result = render_time_boxed(&[], now);
        assert_eq!(empty_result, None);
        assert_ne!(empty_result, Some(String::new()));

        let non_empty = vec![Boxed {
            arm: "arm-a".to_string(),
            expires_at: "2024-01-01T13:00:00Z".to_string(),
            enabled: true,
        }];
        let res = render_time_boxed(&non_empty, now);
        assert!(res.is_some());
        let s = res.unwrap();
        assert!(!s.is_empty());
        assert!(!s.ends_with('\n'));
    }

    #[test]
    fn render_time_boxed_heading_and_ordering() {
        let now = dt("2024-01-01T12:00:00Z");
        let boxed = vec![
            Boxed {
                arm: "first-arm".to_string(),
                expires_at: "2024-01-01T13:00:00Z".to_string(),
                enabled: true,
            },
            Boxed {
                arm: "second-arm".to_string(),
                expires_at: "2024-01-01T11:00:00Z".to_string(),
                enabled: false,
            },
            Boxed {
                arm: "third-arm".to_string(),
                expires_at: "2024-01-01T14:00:00Z".to_string(),
                enabled: true,
            },
        ];
        let out = render_time_boxed(&boxed, now).expect("expected section for non-empty boxed");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0], "== time-boxed arms ==");
        assert!(lines[1].contains("first-arm"));
        assert!(lines[2].contains("second-arm"));
        assert!(lines[3].contains("third-arm"));

        // Ordering is caller's: reversed input preserves reversed order without sorting
        let reversed = vec![boxed[2].clone(), boxed[1].clone(), boxed[0].clone()];
        let out_rev = render_time_boxed(&reversed, now).expect("expected section");
        let lines_rev: Vec<&str> = out_rev.lines().collect();
        assert_eq!(lines_rev.len(), 4);
        assert_eq!(lines_rev[0], "== time-boxed arms ==");
        assert!(lines_rev[1].contains("third-arm"));
        assert!(lines_rev[2].contains("second-arm"));
        assert!(lines_rev[3].contains("first-arm"));
    }

    #[test]
    fn render_time_boxed_must_disable_marking_and_boundaries() {
        let now = dt("2024-01-01T12:00:00Z");

        // Expired arm: enabled true (must_disable true) vs enabled false (must_disable false)
        let exp_enabled = vec![Boxed {
            arm: "arm-exp".to_string(),
            expires_at: "2024-01-01T10:00:00Z".to_string(),
            enabled: true,
        }];
        let exp_disabled = vec![Boxed {
            arm: "arm-exp".to_string(),
            expires_at: "2024-01-01T10:00:00Z".to_string(),
            enabled: false,
        }];
        let out_exp_en = render_time_boxed(&exp_enabled, now).unwrap();
        let out_exp_dis = render_time_boxed(&exp_disabled, now).unwrap();
        let line_exp_en = out_exp_en.lines().nth(1).unwrap();
        let line_exp_dis = out_exp_dis.lines().nth(1).unwrap();
        assert!(line_exp_en.contains("arm-exp"));
        assert!(line_exp_dis.contains("arm-exp"));
        assert_ne!(
            line_exp_en, line_exp_dis,
            "expired enabled arm line must differ from expired disabled arm line"
        );

        // Unparseable timestamp: enabled true (must_disable true) vs enabled false (must_disable false)
        let unp_enabled = vec![Boxed {
            arm: "arm-unp".to_string(),
            expires_at: "not-a-timestamp".to_string(),
            enabled: true,
        }];
        let unp_disabled = vec![Boxed {
            arm: "arm-unp".to_string(),
            expires_at: "not-a-timestamp".to_string(),
            enabled: false,
        }];
        let out_unp_en = render_time_boxed(&unp_enabled, now).unwrap();
        let out_unp_dis = render_time_boxed(&unp_disabled, now).unwrap();
        let line_unp_en = out_unp_en.lines().nth(1).unwrap();
        let line_unp_dis = out_unp_dis.lines().nth(1).unwrap();
        assert!(line_unp_en.contains("arm-unp"));
        assert!(line_unp_dis.contains("arm-unp"));
        assert_ne!(
            line_unp_en, line_unp_dis,
            "unparseable enabled arm line must differ from unparseable disabled arm line"
        );

        // At the exact instant: enabled true (must_disable true) vs enabled false (must_disable false)
        let inst_enabled = vec![Boxed {
            arm: "arm-inst".to_string(),
            expires_at: "2024-01-01T12:00:00Z".to_string(),
            enabled: true,
        }];
        let inst_disabled = vec![Boxed {
            arm: "arm-inst".to_string(),
            expires_at: "2024-01-01T12:00:00Z".to_string(),
            enabled: false,
        }];
        let out_inst_en = render_time_boxed(&inst_enabled, now).unwrap();
        let out_inst_dis = render_time_boxed(&inst_disabled, now).unwrap();
        let line_inst_en = out_inst_en.lines().nth(1).unwrap();
        let line_inst_dis = out_inst_dis.lines().nth(1).unwrap();
        assert!(line_inst_en.contains("arm-inst"));
        assert!(line_inst_dis.contains("arm-inst"));
        assert_ne!(
            line_inst_en, line_inst_dis,
            "at-instant enabled arm line must differ from at-instant disabled arm line"
        );

        // Window open: enabled true (must_disable false)
        let open_arm = vec![Boxed {
            arm: "arm-open".to_string(),
            expires_at: "2024-01-01T13:00:00Z".to_string(),
            enabled: true,
        }];
        let out_open = render_time_boxed(&open_arm, now).unwrap();
        let line_open = out_open.lines().nth(1).unwrap();
        assert!(line_open.contains("arm-open"));
    }

    #[test]
    fn render_time_boxed_empty_arm_name() {
        let now = dt("2024-01-01T12:00:00Z");
        let boxed = vec![Boxed {
            arm: String::new(),
            expires_at: "2024-01-01T13:00:00Z".to_string(),
            enabled: true,
        }];
        let out = render_time_boxed(&boxed, now).expect("empty arm name is degenerate but valid");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "== time-boxed arms ==");
        assert!(!lines[1].is_empty());
        assert!(!out.ends_with('\n'));
    }

    #[test]
    fn render_time_boxed_deterministic_and_pure() {
        let now = dt("2024-01-01T12:00:00Z");
        let boxed = vec![
            Boxed {
                arm: "arm-1".to_string(),
                expires_at: "2024-01-01T11:00:00Z".to_string(),
                enabled: true,
            },
            Boxed {
                arm: "arm-2".to_string(),
                expires_at: "2024-01-01T13:00:00Z".to_string(),
                enabled: false,
            },
        ];
        let run1 = render_time_boxed(&boxed, now);
        let run2 = render_time_boxed(&boxed, now);
        assert_eq!(run1, run2);
    }

    #[test]
    fn render_time_boxed_existing_renderer_unchanged() {
        let rows = Measurement::observed(vec!["farmerbob-abc P0 do a thing".to_string()]);
        let out = render_bd_ready(&rows);
        assert_eq!(
            out,
            "== bd ready (leaf tasks only) ==\n  farmerbob-abc P0 do a thing\n"
        );
    }
}

#[cfg(test)]
mod time_boxed_tests {
    use super::time_boxed;
    use crate::sources::Registry;
    use chrono::{DateTime, Utc};

    fn reg(toml_src: &str) -> Registry {
        toml::from_str(toml_src).expect("fixture parses")
    }
    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s)
            .expect("fixture time")
            .with_timezone(&Utc)
    }

    /// The case the section exists for: the clock says closed and the registry still says
    /// verified. `eligible_at` cannot report this -- `decide` tests `disabled` before the
    /// window and short-circuits -- so asking it would call an expired arm "window open".
    #[test]
    fn an_expired_window_on_a_verified_arm_shouts() {
        let r = reg("[source.a]\nstatus = \"verified\"\nexpires_at = \"2026-01-01T00:00:00Z\"\n");
        let rows = time_boxed(&r, at("2026-01-01T02:30:00Z"));
        assert_eq!(rows.len(), 1);
        assert!(rows[0].contains("EXPIRED"), "{}", rows[0]);
        assert!(rows[0].contains("STILL ENABLED"), "{}", rows[0]);
        assert!(rows[0].contains("2h30m"), "{}", rows[0]);
    }

    /// Same instant, same expiry, one field different: an arm already disabled is reported
    /// calmly. The pair is the point -- the difference is what the operator must act on.
    #[test]
    fn an_expired_window_on_a_disabled_arm_is_calm() {
        let r = reg("[source.a]\nstatus = \"disabled\"\nexpires_at = \"2026-01-01T00:00:00Z\"\n");
        let rows = time_boxed(&r, at("2026-01-01T02:30:00Z"));
        assert!(rows[0].contains("already disabled"), "{}", rows[0]);
        assert!(!rows[0].contains("STILL ENABLED"), "{}", rows[0]);
    }

    /// A window still open reports what is left, not an expiry.
    #[test]
    fn an_open_window_reports_the_time_left() {
        let r = reg("[source.a]\nstatus = \"verified\"\nexpires_at = \"2026-01-01T05:00:00Z\"\n");
        let rows = time_boxed(&r, at("2026-01-01T00:30:00Z"));
        assert!(rows[0].contains("4h30m left"), "{}", rows[0]);
        assert!(!rows[0].contains("EXPIRED"), "{}", rows[0]);
    }

    /// Exactly at the boundary the window is closed, not open. A window that expires at
    /// noon is not usable at noon.
    #[test]
    fn at_the_instant_of_expiry_the_window_is_closed() {
        let r = reg("[source.a]\nstatus = \"verified\"\nexpires_at = \"2026-01-01T00:00:00Z\"\n");
        let rows = time_boxed(&r, at("2026-01-01T00:00:00Z"));
        assert!(rows[0].contains("EXPIRED"), "{}", rows[0]);
    }

    /// An unreadable expiry is treated as EXPIRED, never skipped. The alternative is
    /// dispatching past a window the registry was trying to close.
    #[test]
    fn an_unreadable_expiry_is_treated_as_expired() {
        let r = reg("[source.a]\nstatus = \"verified\"\nexpires_at = \"soon\"\n");
        let rows = time_boxed(&r, at("2026-01-01T00:00:00Z"));
        assert_eq!(rows.len(), 1);
        assert!(rows[0].contains("unreadable"), "{}", rows[0]);
        assert!(rows[0].contains("EXPIRED"), "{}", rows[0]);
    }

    /// An arm with no expires_at is not time-boxed and contributes no row. With none at
    /// all the section is empty and prints no heading.
    #[test]
    fn arms_without_an_expiry_are_not_listed() {
        let r = reg(
            "[source.a]\nstatus = \"verified\"\n[source.b]\nstatus = \"verified\"\nexpires_at = \"2026-01-01T05:00:00Z\"\n",
        );
        let rows = time_boxed(&r, at("2026-01-01T00:00:00Z"));
        assert_eq!(rows.len(), 1);
        assert!(rows[0].contains("b:"), "{}", rows[0]);
        assert!(
            time_boxed(
                &reg("[source.a]\nstatus = \"verified\"\n"),
                at("2026-01-01T00:00:00Z")
            )
            .is_empty()
        );
    }
}
