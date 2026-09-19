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
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

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
    print!("{}", render(&repo, &logs, &live, &waves, &bd_ready.rows));
    bd_ready.exit_code
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

/// The live `fb-<task>--<arm>-<pid>` systemd scopes, `fb-` stripped, deduped and sorted --
/// mirrors `systemctl --user list-units --type=scope` piped through the script's
/// grep/sed/sort. `Missing` only when systemctl itself could not be asked (not installed, or
/// exits non-zero, e.g. no session bus) -- never for "asked, and none are running", which is
/// `Observed(vec![])`.
pub(crate) fn observe_live_agents() -> Measurement<Vec<String>> {
    match Command::new("systemctl")
        .args(["--user", "list-units", "--type=scope", "--no-legend"])
        .output()
    {
        Ok(o) => parse_systemctl_output(o.status.success(), &String::from_utf8_lossy(&o.stdout)),
        Err(e) => Measurement::instrument_failed(&format!("systemctl: {e}")),
    }
}

fn parse_systemctl_output(succeeded: bool, stdout: &str) -> Measurement<Vec<String>> {
    if !succeeded {
        return Measurement::instrument_failed("systemctl exited non-zero");
    }
    let mut names: Vec<String> = stdout.lines().filter_map(extract_scope_name).collect();
    Measurement::observed(sort_unique(&mut names))
}

/// Equivalent of `grep -o 'fb-[a-z0-9-]*--[a-z0-9_-]*' | sed 's/^fb-//'` for one line.
///
/// The two character classes overlap almost entirely (both accept lowercase letters, digits
/// and `-`; only `_` is suffix-only), so the text a leftmost-longest match captures does not
/// depend on exactly where the mandatory `--` splits prefix from suffix -- only on whether one
/// exists at all in the run of matching characters after `fb-`. That means this can extract
/// the match directly instead of implementing a POSIX ERE engine: find `fb-`, extend through
/// every following character in the combined class, and require a literal `--` somewhere in
/// what was extended.
fn extract_scope_name(line: &str) -> Option<String> {
    let start = line.find("fb-")?;
    let after = &line[start + 3..];
    let mut end = 0;
    for c in after.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' {
            end += c.len_utf8();
        } else {
            break;
        }
    }
    let candidate = &after[..end];
    candidate.contains("--").then(|| candidate.to_string())
}

/// Dedup and sort the way `sort -u` does: by the shell's locale collation, not a byte sort.
/// Falls back to a plain sort only if `sort` itself cannot be run, which would already mean
/// something more fundamental than this report is broken.
fn sort_unique(lines: &mut [String]) -> Vec<String> {
    let mut child = match Command::new("sort")
        .arg("-u")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return plain_sort_unique(lines),
    };
    if let Some(mut stdin) = child.stdin.take() {
        for line in lines.iter() {
            if writeln!(stdin, "{line}").is_err() {
                break;
            }
        }
    }
    match child.wait_with_output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(str::to_string)
            .collect(),
        _ => plain_sort_unique(lines),
    }
}

fn plain_sort_unique(lines: &mut [String]) -> Vec<String> {
    let mut v = lines.to_vec();
    v.sort();
    v.dedup();
    v
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
fn target_declaration(spec_path: &Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(spec_path).ok()?;
    for line in text.lines() {
        let Some(inner) = line
            .trim()
            .strip_prefix("<!--")
            .and_then(|r| r.strip_suffix("-->"))
        else {
            continue;
        };
        let tokens: Vec<&str> = inner.split_whitespace().collect();
        let [tag, path] = tokens[..] else { continue };
        let verb = match tag {
            "fb:creates" => "creates",
            "fb:modifies" => "modifies",
            _ => continue,
        };
        return Some((verb.to_string(), path.to_string()));
    }
    None
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

/// Specs with no `.score.json` yet. A `creates` spec whose target already exists is dead --
/// dispatch's precondition rejects it, or every arm correctly no-ops and measures nothing --
/// so it is flagged STALE rather than listed as ordinary available work.  (bead farmerbob-m71)
fn render_unspent_specs(repo: &Path, logs: &Path) -> String {
    let mut s = String::new();
    let prompts_dir = repo.join(".fb/prompts");
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
        let spec_path = prompts_dir.join(&name);
        match target_declaration(&spec_path) {
            Some((verb, path))
                if verb == "creates" && !path.is_empty() && repo.join(&path).is_file() =>
            {
                s.push_str(&format!(
                    "  unspent spec: {b}   ** STALE: {path} already exists, a creates-task would no-op **\n"
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

    // -- extract_scope_name --------------------------------------------------------------

    #[test]
    fn extracts_the_task_and_arm_stripped_of_the_fb_prefix() {
        let line =
            r"fb-port-status--claude-sonnet-2778697.scope                    loaded active running";
        assert_eq!(
            extract_scope_name(line),
            Some("port-status--claude-sonnet-2778697".to_string())
        );
    }

    #[test]
    fn a_line_with_no_fb_prefix_extracts_nothing() {
        let line = "app-Hyprland-gtk\\x2dlaunch-02a14ccd.scope   loaded active running gtk-launch";
        assert_eq!(extract_scope_name(line), None);
    }

    #[test]
    fn a_line_with_no_double_hyphen_extracts_nothing() {
        // "fb-" followed by a run with no "--" is not a task--arm pair.
        assert_eq!(extract_scope_name("fb-onlyoneword.scope loaded"), None);
    }

    #[test]
    fn stops_at_the_first_character_outside_the_class() {
        let name = extract_scope_name("fb-a--b_c.scope tail").unwrap();
        assert_eq!(name, "a--b_c");
    }

    // -- systemctl / pgrep / bd parsing ---------------------------------------------------

    #[test]
    fn systemctl_failure_is_missing_not_zero() {
        let m = parse_systemctl_output(false, "");
        assert!(matches!(
            m,
            Measurement::Missing(Absent::InstrumentFailed { .. })
        ));
    }

    #[test]
    fn systemctl_success_with_no_scopes_is_observed_empty() {
        let m = parse_systemctl_output(true, "");
        assert_eq!(m, Measurement::Observed(vec![]));
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
        assert_eq!(
            out,
            "  unspent spec: dead   ** STALE: already-there.rs already exists, a creates-task would no-op **\n"
        );
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
