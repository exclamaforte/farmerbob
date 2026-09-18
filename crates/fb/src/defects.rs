//! Defect sensitivity: does a candidate's frozen suite CATCH a known defect?
//!
//! Ported from `fb-defects.sh`, whose behaviour is the contract, including its
//! exit codes and its stdout format, because other scripts parse both.
//!
//! The measurement this module exists to take: inject a known defect into the
//! merged reference implementation, run a candidate's frozen suite against it,
//! and count how many defects the suite detects. That is the adjudicator's
//! only DIRECT evidence of suite quality -- everything else is a proxy -- and
//! it has been available for one task in forty-eight.
//!
//! Three facts the shell could not represent, and this module can:
//!
//! - **A suite that did not compile detected nothing.** The script's third
//!   outcome, `nocompile`, is *absence of a measurement*, not a pass with
//!   zero detections. It is carried as [`Measurement::Missing`], never as a
//!   [`Detection`], so it can never be counted as a catch.
//! - **A cell that never ran is not a cell that ran and found nothing.** The
//!   script's python defaults a missing result file to `"nocompile"`, which
//!   silently shrinks a denominator. Here it is
//!   [`Absent::NotAttempted`] and stays out of every denominator for a stated
//!   reason.
//! - **A sensitivity of zero and no sensitivity at all are different rows.**
//!   `0/3` prints `0%`; `0/0` prints `n/a`, because the script prints `None`
//!   as `n/a` and this module keeps the distinction in the type.
//!
//! Every pass/fail verdict here is decided by [`farmerbob_core::gate::judge`],
//! the crate's one implementation of "did this actually run and pass". The
//! orchestration (copying trees, running `cargo`) is separated from the pure
//! core (patching, grafting, classification, aggregation, formatting), which
//! is what the tests pin.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use farmerbob_core::gate::{Observation, Verdict, judge};
use farmerbob_core::measurement::{Absent, Measurement};
use farmerbob_core::sensitivity::Detection;

/// How many `cargo test` cycles run concurrently, when `FB_CX_SLOTS` is unset
/// or unparseable. The script's `SLOTS=${FB_CX_SLOTS:-4}`.
pub const SLOTS_DEFAULT: usize = 4;

/// Per-cell timeout for one `cargo test` run, in seconds: the script's
/// `timeout 300`.
pub const CARGO_TIMEOUT_SECS: u64 = 300;

/// Width of the defect-name column in the validation report, from the script's
/// `printf '  %-28s ...'`.
const NAME_WIDTH: usize = 28;

/// One arm's run against one defect: what the suite did about it, or why
/// nothing was measured.
type CellResult = Measurement<Detection>;

/// Every measured cell, keyed by `(arm, defect)`. A cell that is not in the
/// map was never run, which is a different fact from one that ran and missed.
type CellResults = BTreeMap<(String, String), CellResult>;

/// Public entry point, mirroring the script's positional parameters in order:
/// `bead` (`$1`), `krate` (`$2`), `target` (`$3`).
///
/// Returns the process exit code exactly as the script does: `0` after writing
/// `<logs>/<bead>.defects.json` and printing the summary table; `1` when there
/// is nothing to measure -- no defect was validated and none is beyond the
/// reference -- and `1` for any error that stops the measurement, since the
/// script's only non-zero exit is that one.
pub fn run_cmd(bead: &str, krate: &str, target: &str) -> i32 {
    match run(bead, krate, target) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

/// The real body of [`run_cmd`], returning its exit code or a structured error.
fn run(bead: &str, krate: &str, target: &str) -> Result<i32> {
    let repo = crate::paths::repo();
    let wt_root = crate::paths::worktrees();
    let logs = crate::paths::logs();

    let defects_dir = repo.join(".fb/defects").join(bead);
    let out_path = logs.join(format!("{bead}.defects.json"));

    let tmp = TempDir::new().context("creating a temporary directory")?;

    // The reference: the merged implementation, copied once and copied again
    // per cell so no cell ever sees another's mutation.
    let reference = tmp.path().join("ref");
    copy_reference(&repo, &reference)?;

    // The conformance suite is the INSTRUMENT every defect is validated with.
    // The script would carry on without it -- `sed` on a missing file appends
    // nothing, `cargo test` passes on unmutated code, and every defect is then
    // classified "BEYOND the reference", which reads as a measurement of the
    // field when it is a broken instrument. Refuse instead.
    let conformance = repo.join(".fb/conformance").join(format!("{bead}.rs"));
    let conformance_src = std::fs::read_to_string(&conformance).with_context(|| {
        format!(
            "reading the reference conformance suite {}",
            conformance.display()
        )
    })?;

    println!("validating defects against the reference implementation");

    let mut valid: Vec<String> = Vec::new();
    let mut beyond: Vec<String> = Vec::new();

    for name in list_patches(&defects_dir) {
        let patch = defects_dir.join(format!("{name}.patch"));
        let spec = match std::fs::read_to_string(&patch) {
            Ok(s) => s,
            Err(_) => {
                println!("  {name:<NAME_WIDTH$} SKIP (anchor missing)");
                continue;
            }
        };
        let dest = tmp.path().join(format!("v.{name}"));
        prepare(&reference, &dest)?;

        // The script swallows a failed patch with `2>/dev/null` and prints
        // SKIP. The reason is kept here so a patch that did not apply can be
        // told apart from a defect the reference simply misses.
        if apply_patch_to_file(&spec, &dest.join(target)).is_err() {
            println!("  {name:<NAME_WIDTH$} SKIP (anchor missing)");
            let _ = std::fs::remove_dir_all(&dest);
            continue;
        }

        let measurement = run_cell(&dest, krate, target, &conformance_src);
        let class = classify_defect(&measurement);
        println!("{}", validation_line(&name, class.clone()));
        match class {
            DefectClass::Valid => valid.push(name),
            DefectClass::BeyondReference => beyond.push(name),
            DefectClass::NotADefect(_) => {}
        }
        let _ = std::fs::remove_dir_all(&dest);
    }

    println!(
        "  {} validated defects, {} beyond the reference",
        valid.len(),
        beyond.len()
    );
    if valid.is_empty() && beyond.is_empty() {
        println!("nothing to measure");
        return Ok(1);
    }

    let arms = discover_suites(&wt_root, bead, target, tmp.path());
    println!("  {} candidate suites", arms.len());

    let slots: usize = std::env::var("FB_CX_SLOTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(SLOTS_DEFAULT)
        .max(1);
    let cells = run_cells(
        &Runs {
            reference: &reference,
            tmp: tmp.path(),
            defects_dir: &defects_dir,
            krate,
            target,
        },
        &arms,
        &valid,
        &beyond,
        slots,
    )?;

    let rows = compute_rows(&arms, &valid, &beyond, &|arm, defect| {
        cells
            .get(&(arm.to_string(), defect.to_string()))
            .cloned()
            // The script defaults a missing result file to "nocompile". It is
            // excluded from every denominator either way; here it says why.
            .unwrap_or_else(Measurement::not_attempted)
    });

    print!("{}", format_table(&rows, &beyond));

    std::fs::create_dir_all(&logs).with_context(|| format!("creating {}", logs.display()))?;
    write_json(&out_path, valid.len(), beyond.len(), &rows)
        .with_context(|| format!("writing {}", out_path.display()))?;
    println!("-> {}", out_path.display());

    Ok(0)
}

// ---------------------------------------------------------------------------
// Pure core: patching, freezing, grafting
// ---------------------------------------------------------------------------

/// Why a defect patch could not be applied to the reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchError {
    /// A block of the spec does not hold exactly one `--->` separator, so it
    /// cannot be read as `old ---> new`. The script's python raises here and
    /// the defect is skipped.
    Malformed {
        /// Index of the offending block, zero-based, counting only the blocks
        /// after the leading `@@@`.
        block: usize,
    },
    /// The `old` text is not present in the source: the anchor has moved.
    /// Distinct from a defect the reference merely misses -- a patch that did
    /// not apply tells us nothing at all.
    AnchorMissing {
        /// Index of the block whose anchor is missing.
        block: usize,
    },
}

/// Split a patch spec into `(old, new)` blocks, exactly as the script's python
/// does: the text is split on `@@@`, the first piece is dropped, and every
/// remaining block is split on `--->` into exactly two parts, each stripped of
/// leading and trailing newlines.
///
/// A block with no `--->`, or with more than one, is [`PatchError::Malformed`].
pub fn parse_patch(spec: &str) -> Result<Vec<(String, String)>, PatchError> {
    let mut blocks = Vec::new();
    for (i, block) in spec.split("@@@").skip(1).enumerate() {
        let mut parts = block.split("--->");
        let old = parts.next().unwrap_or_default();
        let new = parts.next().ok_or(PatchError::Malformed { block: i })?;
        if parts.next().is_some() {
            return Err(PatchError::Malformed { block: i });
        }
        blocks.push((strip_newlines(old), strip_newlines(new)));
    }
    Ok(blocks)
}

/// `str.strip('\n')`: newlines only, from both ends. Spaces are part of the
/// anchor and must survive.
fn strip_newlines(s: &str) -> String {
    s.trim_matches('\n').to_string()
}

/// Apply parsed blocks to `src`, replacing the FIRST occurrence of each `old`
/// with its `new`, in order, so a later block sees the earlier ones' edits --
/// the script's `src.replace(old, new, 1)` in a loop.
///
/// All-or-nothing, like the script: its python `sys.exit(2)`s before writing,
/// so a half-applied patch never reaches the file.
pub fn apply_blocks(src: &str, blocks: &[(String, String)]) -> Result<String, PatchError> {
    let mut out = src.to_string();
    for (i, (old, new)) in blocks.iter().enumerate() {
        if old.is_empty() {
            // Python's `"abc".replace("", "X", 1)` inserts at offset 0 rather
            // than failing. Preserved, so a malformed patch fails the same way
            // here as it does there.
            out.insert_str(0, new);
            continue;
        }
        match out.find(old.as_str()) {
            Some(at) => {
                out.replace_range(at..at + old.len(), new);
            }
            None => return Err(PatchError::AnchorMissing { block: i }),
        }
    }
    Ok(out)
}

/// Parse and apply a patch spec in one step.
pub fn apply_patch(src: &str, spec: &str) -> Result<String, PatchError> {
    let blocks = parse_patch(spec)?;
    apply_blocks(src, &blocks)
}

/// Freeze a candidate's own suite: keep everything from the first
/// `#[cfg(test)]` line onwards, rename `mod tests` to `mod frozen`, and rewrite
/// `use super::*;` as `use crate::<module>::*;`.
///
/// That last rewrite is what lets the suite be grafted into the reference: as a
/// child of the module it tests, `super::` resolves to its own crate's parent,
/// and the reference's copy of that module has a different one.
pub fn freeze_suite(src: &str, module: &str) -> String {
    let replacement = format!("use crate::{module}::*;");
    let mut out = String::new();
    let mut in_tests = false;
    for line in src.lines() {
        if !in_tests && line.contains("#[cfg(test)]") {
            in_tests = true;
        }
        if !in_tests {
            continue;
        }
        let renamed = rename_tests_module(line);
        out.push_str(&renamed.replacen("use super::*;", &replacement, 1));
        out.push('\n');
    }
    out
}

/// `sed 's/^\( *\)mod tests/\1mod frozen/'`: a line beginning with optional
/// spaces then `mod tests` becomes the same line with `mod frozen`.
fn rename_tests_module(line: &str) -> String {
    let spaces = leading_spaces(line);
    match line[spaces..].strip_prefix("mod tests") {
        Some(tail) => format!("{}mod frozen{}", &line[..spaces], tail),
        None => line.to_string(),
    }
}

/// Rename every grafted module: `sed 's/^\( *\)mod \([a-z_0-9]*\)/\1mod dfx_\2/'`.
///
/// The merged reference already contains this very suite -- it was escalated
/// into the file when the task was adjudicated -- so appending it verbatim is
/// "the name `cx_budget_codex_luna` is defined multiple times", which reads as
/// `nocompile` and excludes the defect exactly as an import failure would.
/// The rename is what makes the graft compile at all.
///
/// Only the first match on each line is rewritten (the sed has no `/g`), and
/// the capture is `[a-z_0-9]*`, which may be empty: `mod Tests {` becomes
/// `mod dfx_Tests {`.
pub fn rename_grafted_modules(suite: &str) -> String {
    let mut out = String::new();
    for line in suite.lines() {
        let spaces = leading_spaces(line);
        match line[spaces..].strip_prefix("mod ") {
            Some(after) => {
                let name_len = after
                    .bytes()
                    .take_while(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_')
                    .count();
                let (name, tail) = after.split_at(name_len);
                out.push_str(&line[..spaces]);
                out.push_str("mod dfx_");
                out.push_str(name);
                out.push_str(tail);
            }
            None => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

/// Count of leading spaces on `line`. The script's sed captures `\( *\)`,
/// spaces only -- a tab-indented line is not a match.
fn leading_spaces(line: &str) -> usize {
    line.bytes().take_while(|b| *b == b' ').count()
}

/// True when `output` shows a compile failure: a line beginning
/// `error[E<digits>]:`, or the text `could not compile`. The script's
/// `grep -qE '^error\[E[0-9]+\]:|could not compile'`.
pub fn is_compile_error(output: &str) -> bool {
    if output.contains("could not compile") {
        return true;
    }
    for line in output.lines() {
        // `^error\[E[0-9]+\]:` -- digits, then the closing bracket, then the
        // colon. All three, or it is not a compiler error code.
        if let Some(rest) = line.strip_prefix("error[E") {
            let digits = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
            let bytes = rest.as_bytes();
            if digits > 0
                && bytes.get(digits) == Some(&b']')
                && bytes.get(digits + 1) == Some(&b':')
            {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Pure core: classification and aggregation
// ---------------------------------------------------------------------------

/// One `cargo test` run of a grafted suite, as the gate needs to see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoRun {
    /// `cargo test` exited 0.
    pub ok: bool,
    /// The run was killed -- in practice, by the timeout. A killed run is not
    /// a detection: the suite never finished, so it detected nothing.
    pub killed: bool,
    /// Whether the output shows a compile failure, per [`is_compile_error`].
    pub compile_error: bool,
    /// How many `#[test]` functions the grafted suite declares.
    pub tests_declared: u32,
    /// How many lines the graft added to the host module.
    pub graft_lines: u32,
}

/// Classify one run as a [`Detection`] measurement, via the crate's gate.
///
/// A suite that FAILED detected the defect ([`Detection::Caught`], the
/// script's `fail`); one that PASSED missed it ([`Detection::Missed`], the
/// script's `pass`). Anything else is not a measurement at all:
///
/// - did not compile -- [`Absent::InstrumentFailed`], the script's
///   `nocompile`, excluded from every denominator;
/// - killed, or declared no tests, or grafted no lines --
///   nothing was measured, for the stated reason.
///
/// [`Absent::InstrumentFailed`]: farmerbob_core::measurement::Absent::InstrumentFailed
pub fn classify_run(run: &CargoRun) -> Measurement<Detection> {
    if run.killed {
        return Measurement::instrument_failed("cargo test was killed before it finished");
    }
    let observation = Observation {
        built: Some(!run.compile_error),
        tests_passed: Some(run.ok),
        tests_run: Some(if run.compile_error {
            0
        } else {
            run.tests_declared
        }),
        // The graft is what was added to the declared target; a graft of no
        // lines added nothing, and the gate must not read that as a pass.
        lines_added: Some(run.graft_lines),
        // Checked before the run: the host module exists or there is no cell.
        declared_targets_present: Some(true),
        // Synthetic observation: `judge` used as a boolean combinator, not to score a
        // run. No worktree, so no scope to depart from. Some(0) rather than None
        // deliberately -- None means "not assessed" and yields Indeterminate, which
        // here would turn a correct answer into a refusal to answer.
        scope_departures: Some(0),
    };
    verdict_to_detection(judge(&observation))
}

/// Map the gate's verdict onto a detection measurement.
fn verdict_to_detection(v: Verdict) -> Measurement<Detection> {
    match v {
        // The suite passed against the defect: it missed it.
        Verdict::Pass => Measurement::observed(Detection::Missed),
        // The suite failed against the defect: it caught it.
        Verdict::TestsFail => Measurement::observed(Detection::Caught),
        // nocompile. A suite that cannot build has not detected anything.
        Verdict::NoCompile => Measurement::instrument_failed("the grafted suite did not compile"),
        Verdict::NoTests => Measurement::nothing_to_measure("the grafted suite declares no tests"),
        Verdict::NoOp => Measurement::nothing_to_measure("the grafted suite contributed no lines"),
        Verdict::WrongTarget => Measurement::instrument_failed("the target module is absent"),
        Verdict::Indeterminate => Measurement::not_attempted(),
        // Unreachable from the synthetic observation above, which always supplies Some(0),
        // but stated rather than caught by a wildcard: a wildcard here would silently absorb
        // any future verdict, which is how a new state becomes an old one's behaviour.
        Verdict::OutOfScope => {
            Measurement::instrument_failed("a grafted suite cannot depart its own scope")
        }
    }
}

/// What a defect's validation against the reference conformance suite says
/// about the defect. Exactly these three, and they are exclusive: a defect is
/// never two of these at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefectClass {
    /// The reference suite detects it: a real, spec-relevant defect.
    Valid,
    /// The reference suite MISSES it. Kept and scored separately: a candidate
    /// suite that catches one has proved itself strictly better than the
    /// reference. An earlier version of the script discarded these as inert,
    /// and they are the only mutants that can discriminate between candidates.
    BeyondReference,
    /// It did not compile, or was never measured at all: not a defect.
    NotADefect(Absent),
}

impl DefectClass {
    /// The word the report prints for this class, after the defect name.
    fn suffix(&self) -> String {
        match self {
            DefectClass::Valid => "valid (conformance detects it)".to_string(),
            DefectClass::BeyondReference => "BEYOND the reference (it misses this one)".to_string(),
            DefectClass::NotADefect(absent) => {
                format!("excluded ({}) -- not a defect", absent_tag(absent))
            }
        }
    }
}

/// The short word the report prints for an absent measurement, in the
/// `excluded (%s) -- not a defect` line. `nocompile` is the script's own word
/// for a suite that did not build and is kept verbatim; the others name
/// themselves, because the script had only the one.
pub fn absent_tag(absent: &Absent) -> &'static str {
    match absent {
        Absent::InstrumentFailed { .. } => "nocompile",
        Absent::NotAttempted => "notrun",
        Absent::NothingToMeasure { .. } => "nothing-to-measure",
        Absent::Untrusted { .. } => "untrusted",
    }
}

/// Classify one defect against the reference conformance suite.
///
/// [`Detection::Caught`] means the reference caught it ([`DefectClass::Valid`]);
/// [`Detection::Missed`] means the reference missed it
/// ([`DefectClass::BeyondReference`]); any absent measurement means the
/// mutation is not a defect at all.
pub fn classify_defect(measurement: &Measurement<Detection>) -> DefectClass {
    match measurement.value().copied() {
        Some(Detection::Caught) => DefectClass::Valid,
        Some(Detection::Missed) => DefectClass::BeyondReference,
        _ => {
            let absent = measurement
                .absent()
                .cloned()
                .unwrap_or(Absent::NotAttempted);
            DefectClass::NotADefect(absent)
        }
    }
}

/// Render one validation line: the script's `printf '  %-28s %s\n'`.
pub fn validation_line(name: &str, class: DefectClass) -> String {
    format!("  {name:<NAME_WIDTH$} {}", class.suffix())
}

/// One candidate suite's defect-sensitivity result: everything the summary
/// table and the JSON report need for a single arm.
///
/// `sensitivity` is a [`Measurement`] rather than a bare `f64`: when no
/// validated defect produced a measurement for this suite there is no fraction
/// to compute, and reporting `0.0` would be the lie the script avoided only in
/// its printing (`None` became `n/a`). Here the absence is in the type.
pub struct SuiteRow {
    /// The arm whose suite this row describes.
    pub arm: String,
    /// Validated defects this suite detected.
    pub caught: usize,
    /// Validated defects that produced a measurement for this suite; the
    /// denominator. `nocompile` and never-run cells are absent from it.
    pub comparable: usize,
    /// `caught / comparable`, or `Missing` when `comparable` is 0.
    pub sensitivity: Measurement<f64>,
    /// Defects the reference misses that this suite nonetheless detects.
    /// Catching one proves this suite strictly better than the reference.
    pub beyond_caught: usize,
    /// Defects beyond the reference that produced a measurement here.
    pub beyond_of: usize,
    /// Validated defects this suite ran against and did not detect, in the
    /// order the defects appear in the validated set.
    pub missed: Vec<String>,
}

/// Fold every `(arm, defect)` cell into per-arm rows.
///
/// `verdict` answers one cell; a cell it has no answer for is unmeasured, and
/// an unmeasured cell leaves EVERY denominator it would have entered -- it is
/// not a miss, and it is not a catch. Rows come back in `arms` order.
pub fn compute_rows(
    arms: &[String],
    valid: &[String],
    beyond: &[String],
    verdict: &dyn Fn(&str, &str) -> Measurement<Detection>,
) -> Vec<SuiteRow> {
    let mut rows = Vec::new();
    for arm in arms {
        let mut caught = 0usize;
        let mut comparable = 0usize;
        let mut missed = Vec::new();
        for name in valid {
            match verdict(arm, name).value().copied() {
                Some(Detection::Caught) => {
                    comparable += 1;
                    caught += 1;
                }
                Some(Detection::Missed) => {
                    comparable += 1;
                    missed.push(name.clone());
                }
                // DidNotRun and every absence: no measurement, so this defect
                // leaves this suite's denominator.
                Some(Detection::DidNotRun) | None => {}
            }
        }
        let sensitivity = if comparable == 0 {
            Measurement::nothing_to_measure(
                "no validated defect produced a measurement for this suite",
            )
        } else {
            Measurement::observed(caught as f64 / comparable as f64)
        };

        let mut beyond_caught = 0usize;
        let mut beyond_of = 0usize;
        for name in beyond {
            match verdict(arm, name).value().copied() {
                Some(Detection::Caught) => {
                    beyond_of += 1;
                    beyond_caught += 1;
                }
                Some(Detection::Missed) => beyond_of += 1,
                Some(Detection::DidNotRun) | None => {}
            }
        }

        rows.push(SuiteRow {
            arm: arm.clone(),
            caught,
            comparable,
            sensitivity,
            beyond_caught,
            beyond_of,
            missed,
        });
    }
    rows
}

/// The script's sort key: `-(sensitivity or -1)`, where Python's `or` is
/// truthiness, so a sensitivity of `0.0` sorts exactly like an absent one.
fn sort_key(row: &SuiteRow) -> f64 {
    match row.sensitivity.value().copied() {
        Some(v) if v != 0.0 => -v,
        _ => 1.0,
    }
}

/// Render the summary table, byte-for-byte with the script's python: a leading
/// blank line, the header, one row per arm, and -- only when some defect is
/// beyond the reference -- the three lines explaining that column.
///
/// Rows are ordered by `beyond_reference_caught` descending, then by the
/// script's sort key ascending, which puts the highest sensitivity first and
/// ties a measured `0%` with an unmeasured `n/a`. The sort is stable, so ties
/// keep `arms` order.
pub fn format_table(rows: &[SuiteRow], beyond: &[String]) -> String {
    let mut out = String::new();
    out.push('\n');
    out.push_str(&format!(
        "{:<24}{:>9}{:>5}{:>13}{:>12}\n",
        "SUITE", "CAUGHT", "OF", "SENSITIVITY", "BEYOND REF"
    ));

    let mut ordered: Vec<&SuiteRow> = rows.iter().collect();
    ordered.sort_by(|a, b| {
        b.beyond_caught.cmp(&a.beyond_caught).then(
            sort_key(a)
                .partial_cmp(&sort_key(b))
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });

    for r in ordered {
        let s = match r.sensitivity.value().copied() {
            Some(v) => format!("{:.0}%", 100.0 * v),
            None => "n/a".to_string(),
        };
        let b = if r.beyond_of == 0 {
            "-".to_string()
        } else {
            format!("{}/{}", r.beyond_caught, r.beyond_of)
        };
        out.push_str(&format!(
            "{:<24}{:>9}{:>5}{:>13}{:>12}\n",
            r.arm, r.caught, r.comparable, s, b
        ));
    }

    if !beyond.is_empty() {
        out.push('\n');
        out.push_str(
            "  BEYOND REF counts defects the reference conformance suite does NOT detect.\n",
        );
        out.push_str(
            "  Catching one proves a suite is strictly better than the reference; it is the\n",
        );
        out.push_str(
            "  only column here that can separate a field where everyone catches the rest.\n",
        );
    }
    out
}

/// The report the script writes to `<logs>/<bead>.defects.json`, in the
/// script's key order and with its `indent=1` layout.
fn report_json(nvalid: usize, beyond_len: usize, rows: &[SuiteRow]) -> Json {
    let arms: Vec<(String, Json)> = rows
        .iter()
        .map(|r| {
            (
                r.arm.clone(),
                Json::Obj(vec![
                    ("caught".to_string(), Json::Int(r.caught)),
                    ("comparable".to_string(), Json::Int(r.comparable)),
                    (
                        "sensitivity".to_string(),
                        match r.sensitivity.value().copied() {
                            Some(v) => Json::Float(v),
                            None => Json::Null,
                        },
                    ),
                    (
                        "beyond_reference_caught".to_string(),
                        Json::Int(r.beyond_caught),
                    ),
                    ("beyond_reference_of".to_string(), Json::Int(r.beyond_of)),
                    (
                        "missed".to_string(),
                        Json::Arr(r.missed.iter().map(|m| Json::Str(m.clone())).collect()),
                    ),
                ]),
            )
        })
        .collect();

    Json::Obj(vec![
        ("validated_defects".to_string(), Json::Int(nvalid)),
        (
            "beyond_reference_defects".to_string(),
            Json::Int(beyond_len),
        ),
        ("arms".to_string(), Json::Obj(arms)),
    ])
}

/// Write the JSON report. Insertion order is preserved -- the arms keep the
/// order they were measured in -- because `serde_json`'s map sorts its keys and
/// other scripts read this file by arm name.
fn write_json(path: &Path, nvalid: usize, beyond_len: usize, rows: &[SuiteRow]) -> Result<()> {
    let text = report_json(nvalid, beyond_len, rows).dump(0);
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// A JSON value small enough to write by hand, so the output is byte-for-byte
/// what the script's `json.dump(..., indent=1)` produces: one space per level,
/// `": "` between key and value, `{}` and `[]` for empty containers.
enum Json {
    Null,
    Int(usize),
    Float(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn dump(&self, depth: usize) -> String {
        let inner = " ".repeat(depth + 1);
        let outer = " ".repeat(depth);
        match self {
            Json::Null => "null".to_string(),
            Json::Int(n) => n.to_string(),
            // `serde_json` prints a float the way Python does: 1.0, not 1.
            Json::Float(v) => serde_json::to_string(v).unwrap_or_else(|_| "null".to_string()),
            Json::Str(s) => json_string(s),
            Json::Arr(items) => {
                if items.is_empty() {
                    return "[]".to_string();
                }
                let body: Vec<String> = items.iter().map(|i| i.dump(depth + 1)).collect();
                format!(
                    "[\n{}{}\n{outer}]",
                    inner,
                    body.join(&format!(",\n{inner}")),
                )
            }
            Json::Obj(entries) => {
                if entries.is_empty() {
                    return "{}".to_string();
                }
                let body: Vec<String> = entries
                    .iter()
                    .map(|(k, v)| format!("{}: {}", json_string(k), v.dump(depth + 1)))
                    .collect();
                format!(
                    "{{\n{}{}\n{outer}}}",
                    inner,
                    body.join(&format!(",\n{inner}"))
                )
            }
        }
    }
}

/// Quote a string for JSON. Non-ASCII is passed through rather than escaped as
/// Python would; every name here comes from a directory or a file name.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------
// Orchestration: files, cargo, threads
// ---------------------------------------------------------------------------

/// The stem of the target path: `basename "$TARGET" .rs`. It is both the host
/// module's file name and the module the frozen suite imports.
fn module_stem(target: &str) -> String {
    let base = target.rsplit('/').next().unwrap_or(target);
    base.strip_suffix(".rs").unwrap_or(base).to_string()
}

/// Copy the reference out of the repo: `Cargo.toml`, `Cargo.lock`,
/// `rustfmt.toml` and `crates/`. The script tar-pipes these with `2>/dev/null`,
/// so a missing `crates/` gave it a reference with nothing in it and every
/// defect came back `nocompile`. Fail instead.
fn copy_reference(repo: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest).with_context(|| format!("creating {}", dest.display()))?;
    for name in ["Cargo.toml", "Cargo.lock", "rustfmt.toml"] {
        let src = repo.join(name);
        if src.is_file() {
            std::fs::copy(&src, dest.join(name))
                .with_context(|| format!("copying {}", src.display()))?;
        }
    }
    let crates = repo.join("crates");
    if !crates.is_dir() {
        anyhow::bail!(
            "the reference has no crates directory: {}",
            crates.display()
        );
    }
    copy_dir(&crates, &dest.join("crates"))?;
    Ok(())
}

/// Recursively copy a directory tree.
fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).with_context(|| format!("creating {}", to.display()))?;
    for entry in std::fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let entry = entry?;
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &target)?;
        } else {
            std::fs::copy(&path, &target).with_context(|| format!("copying {}", path.display()))?;
        }
    }
    Ok(())
}

/// Reset `dest` to a fresh copy of the reference: the script's
/// `rm -rf "$d"; cp -r "$REF" "$d"`.
fn prepare(reference: &Path, dest: &Path) -> Result<()> {
    let _ = std::fs::remove_dir_all(dest);
    copy_dir(reference, dest)
}

/// Defect names in `$REPO/.fb/defects/$BEAD`, sorted: the script globs
/// `*.patch`, which is sorted, and takes the basename minus the extension.
fn list_patches(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return names,
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(stem) = name.strip_suffix(".patch") {
            names.push(stem.to_string());
        }
    }
    names.sort();
    names
}

/// Discover candidate suites: every `$WT_ROOT/$BEAD--*` that is a directory,
/// is not a live run, and whose declared target is a non-empty file. Each
/// surviving arm's suite is frozen into `<tmp>/s.<arm>.rs`.
///
/// An arm is kept only if its frozen suite declares at least one `#[test]` --
/// the script's `grep -c '#\[test\]'` -- because a suite with no tests cannot
/// detect anything and would otherwise be counted as having missed every
/// defect.
fn discover_suites(wt_root: &Path, bead: &str, target: &str, tmp: &Path) -> Vec<String> {
    let prefix = format!("{bead}--");
    let mut candidates = Vec::new();
    let entries = match std::fs::read_dir(wt_root) {
        Ok(e) => e,
        Err(_) => return candidates,
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(&prefix) {
            continue;
        }
        let path = entry.path();
        if !path.is_dir() || path.join(".fb-task.md").is_file() {
            continue;
        }
        if !nonempty_file(&path.join(target)) {
            continue;
        }
        candidates.push(name[prefix.len()..].to_string());
    }
    candidates.sort();

    let stem = module_stem(target);
    let mut arms = Vec::new();
    for arm in candidates {
        let file = wt_root.join(format!("{prefix}{arm}")).join(target);
        let src = match std::fs::read_to_string(&file) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let frozen = freeze_suite(&src, &stem);
        if !frozen.lines().any(|l| l.contains("#[test]")) {
            continue;
        }
        if std::fs::write(tmp.join(format!("s.{arm}.rs")), frozen).is_err() {
            continue;
        }
        arms.push(arm);
    }
    arms
}

/// Whether `path` is a file with something in it: the script's `[ -s ... ]`.
fn nonempty_file(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(m) => m.is_file() && m.len() > 0,
        Err(_) => false,
    }
}

/// Read a patch and write it over `dest`'s copy of the target file.
/// All-or-nothing: the script's python writes only after every block applied.
fn apply_patch_to_file(spec: &str, dest_file: &Path) -> Result<(), PatchError> {
    let src =
        std::fs::read_to_string(dest_file).map_err(|_| PatchError::AnchorMissing { block: 0 })?;
    let patched = apply_patch(&src, spec)?;
    std::fs::write(dest_file, patched).map_err(|_| PatchError::AnchorMissing { block: 0 })
}

/// Everything a cell needs that does not vary per cell, gathered so the
/// worker threads can be handed one value instead of five.
struct Runs<'a> {
    /// The pristine copy of the merged reference.
    reference: &'a Path,
    /// Where per-cell copies and frozen suites live.
    tmp: &'a Path,
    /// `$REPO/.fb/defects/$BEAD`.
    defects_dir: &'a Path,
    /// The crate under test.
    krate: &'a str,
    /// The declared deliverable, repo-relative.
    target: &'a str,
}

/// Run every `(arm, defect)` cell, at most `slots` at a time, and return the
/// measurement for each.
fn run_cells(
    runs: &Runs,
    arms: &[String],
    valid: &[String],
    beyond: &[String],
    slots: usize,
) -> Result<CellResults> {
    let defects: Vec<String> = valid.iter().chain(beyond.iter()).cloned().collect();
    let mut tasks: Vec<(String, String)> = Vec::new();
    for arm in arms {
        for name in &defects {
            tasks.push((arm.clone(), name.clone()));
        }
    }

    let workers = slots.min(tasks.len().max(1));

    let results: std::sync::Arc<std::sync::Mutex<CellResults>> =
        std::sync::Arc::new(std::sync::Mutex::new(CellResults::new()));
    let queue: std::sync::Arc<std::sync::Mutex<std::vec::IntoIter<(String, String)>>> =
        std::sync::Arc::new(std::sync::Mutex::new(tasks.into_iter()));
    let mut handles = Vec::new();
    for _ in 0..workers {
        let results = std::sync::Arc::clone(&results);
        let queue = std::sync::Arc::clone(&queue);
        let reference = runs.reference.to_path_buf();
        let tmp = runs.tmp.to_path_buf();
        let defects_dir = runs.defects_dir.to_path_buf();
        let krate = runs.krate.to_string();
        let target = runs.target.to_string();
        let handle = std::thread::Builder::new()
            .name("defects-worker".to_string())
            .spawn(move || {
                loop {
                    let task = match queue.lock() {
                        Ok(mut g) => g.next(),
                        Err(_) => break,
                    };
                    let Some((arm, name)) = task else { break };
                    let cell =
                        run_one(&reference, &tmp, &defects_dir, &krate, &target, &arm, &name);
                    if let Ok(mut r) = results.lock() {
                        r.insert((arm, name), cell);
                    }
                }
            })
            .context("spawning a defect worker")?;
        handles.push(handle);
    }
    for h in handles {
        let _ = h.join();
    }

    let locked = std::sync::Arc::try_unwrap(results)
        .map_err(|_| anyhow::anyhow!("results still shared after every worker joined"))?;
    locked
        .into_inner()
        .map_err(|_| anyhow::anyhow!("a defect worker poisoned the results"))
}

/// One cell: copy the reference, inject `name`, run this arm's frozen suite.
///
/// A patch that does not apply leaves the code UNMUTATED, and the script
/// carried on regardless -- `apply ... 2>/dev/null` with no `set -e` -- so the
/// suite passed against correct code and the defect was recorded as missed.
/// That is a failure becoming a zero, so it is recorded as an absent
/// measurement instead.
fn run_one(
    reference: &Path,
    tmp: &Path,
    defects_dir: &Path,
    krate: &str,
    target: &str,
    arm: &str,
    name: &str,
) -> Measurement<Detection> {
    let dest = tmp.join(format!("r.{arm}.{name}"));
    if prepare(reference, &dest).is_err() {
        return Measurement::instrument_failed("could not prepare a copy of the reference");
    }

    let spec = match std::fs::read_to_string(defects_dir.join(format!("{name}.patch"))) {
        Ok(s) => s,
        Err(_) => return Measurement::instrument_failed("the defect patch could not be read"),
    };
    if let Err(e) = apply_patch_to_file(&spec, &dest.join(target)) {
        let reason = match e {
            PatchError::AnchorMissing { .. } => "the defect patch does not apply to the reference",
            PatchError::Malformed { .. } => "the defect patch is malformed",
        };
        return Measurement::instrument_failed(reason);
    }

    let suite = match std::fs::read_to_string(tmp.join(format!("s.{arm}.rs"))) {
        Ok(s) => s,
        Err(_) => return Measurement::instrument_failed("the frozen suite could not be read"),
    };

    let cell = run_cell(&dest, krate, target, &suite);
    let _ = std::fs::remove_dir_all(&dest);
    cell
}

/// Graft `suite` into the host module of `dest` and run `cargo test`.
///
/// The host is the TARGET MODULE, not lib.rs. A conformance suite is written as
/// a child of the module it tests, so it says `use super::Admission`; appended
/// to lib.rs, `super::` resolves to the crate root and every import fails, so
/// every defect reads `nocompile` and the run ends "0 validated defects" for
/// any task whose deliverable is a module rather than lib.rs itself. That is
/// why this measure has been available for one task in forty-eight.
fn run_cell(dest: &Path, krate: &str, target: &str, suite: &str) -> Measurement<Detection> {
    let src_dir = dest.join(format!("crates/{krate}/src"));
    let host = match host_module(&src_dir, target) {
        Some(h) => h,
        None => {
            return Measurement::instrument_failed(
                "neither the target module nor lib.rs exists to graft into",
            );
        }
    };
    let original = match std::fs::read_to_string(&host) {
        Ok(s) => s,
        Err(_) => return Measurement::instrument_failed("the host module could not be read"),
    };

    // Renamed, because the merged reference already contains this suite.
    let graft = rename_grafted_modules(suite);
    let tests_declared = graft.lines().filter(|l| l.contains("#[test]")).count() as u32;
    let graft_lines = graft.lines().count() as u32;

    let mut merged = original;
    merged.push_str(&graft);
    if std::fs::write(&host, merged).is_err() {
        return Measurement::instrument_failed("the grafted suite could not be written");
    }

    let run = match run_cargo_test(dest, krate, Duration::from_secs(CARGO_TIMEOUT_SECS)) {
        Ok(r) => r,
        Err(e) => {
            return Measurement::instrument_failed(&format!("cargo test could not be run: {e}"));
        }
    };
    classify_run(&CargoRun {
        ok: run.ok,
        killed: run.killed,
        compile_error: is_compile_error(&run.output),
        tests_declared,
        graft_lines,
    })
}

/// The module the suite is grafted into: the target module if it exists in
/// this copy, and lib.rs otherwise. The script's
/// `host="$sd/$(basename "$TARGET")"; [ -f "$host" ] || host="$sd/lib.rs"`.
fn host_module(src_dir: &Path, target: &str) -> Option<PathBuf> {
    let base = target.rsplit('/').next().unwrap_or(target);
    let host = src_dir.join(base);
    if host.is_file() {
        return Some(host);
    }
    let lib = src_dir.join("lib.rs");
    if lib.is_file() { Some(lib) } else { None }
}

/// The result of one `cargo test` run.
struct RunOutput {
    /// The process exited 0.
    ok: bool,
    /// The process was killed by a signal -- the timeout fired.
    killed: bool,
    /// Combined stdout and stderr, which is what the script greps.
    output: String,
}

/// Run `cargo test -p <krate>` in `dir`, killing the process group if it
/// outlives `timeout`.
///
/// The killer thread is CANCELLABLE. A version that slept the whole timeout
/// and was then joined blocked every cell for 300 seconds whether or not cargo
/// had finished, and fired `kill -9` at a process group the child had already
/// left.
fn run_cargo_test(dir: &Path, krate: &str, timeout: Duration) -> std::io::Result<RunOutput> {
    let mut cmd = Command::new("cargo");
    cmd.args(["test", "-p", krate])
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let child = cmd.spawn()?;
    let pid = child.id();

    let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = std::sync::Arc::clone(&finished);
    let killer = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if flag.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        #[cfg(unix)]
        {
            let _ = Command::new("kill")
                .args(["-9", &format!("-{pid}")])
                .status();
        }
        #[cfg(not(unix))]
        {
            let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
        }
    });

    let output = child.wait_with_output()?;
    finished.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = killer.join();

    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(RunOutput {
        ok: output.status.success(),
        killed: killed_by_signal(&output.status),
        output: combined,
    })
}

/// Whether the process died from a signal rather than exiting.
#[cfg(unix)]
fn killed_by_signal(status: &std::process::ExitStatus) -> bool {
    use std::os::unix::process::ExitStatusExt;
    status.signal().is_some()
}

/// Whether the process died from a signal rather than exiting.
#[cfg(not(unix))]
fn killed_by_signal(_status: &std::process::ExitStatus) -> bool {
    false
}

/// A temporary directory removed when dropped.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Result<Self> {
        let path =
            std::env::temp_dir().join(format!("fb-defects-{}-{}", std::process::id(), counter()));
        std::fs::create_dir_all(&path).with_context(|| format!("creating {}", path.display()))?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Process-local counter for unique temporary directory names.
fn counter() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static C: AtomicU64 = AtomicU64::new(0);
    C.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(map: &[(&str, &str, Detection)]) -> Vec<(String, String, Measurement<Detection>)> {
        map.iter()
            .map(|(a, d, v)| {
                (
                    (*a).to_string(),
                    (*d).to_string(),
                    Measurement::observed(*v),
                )
            })
            .collect()
    }

    /// A verdict function over an in-memory table of cells. Anything it has no
    /// answer for is unmeasured, exactly as a missing result file is.
    fn lookup(
        table: &[(String, String, Measurement<Detection>)],
    ) -> impl Fn(&str, &str) -> Measurement<Detection> + '_ {
        move |arm, defect| {
            table
                .iter()
                .find(|(a, d, _)| a == arm && d == defect)
                .map(|(_, _, m)| m.clone())
                .unwrap_or_else(Measurement::not_attempted)
        }
    }

    // --- patching ----------------------------------------------------------

    #[test]
    fn patch_replaces_each_anchor_once() {
        let spec = "@@@\nfn a() {\n--->\nfn a2() {\n@@@1\n--->2";
        let got = apply_patch("fn a() {\n    1\n}", spec).unwrap();
        assert_eq!(got, "fn a2() {\n    2\n}");
    }

    #[test]
    fn patch_blocks_see_earlier_edits() {
        // The second anchor only exists after the first replacement.
        let spec = "@@@x\n--->y@@@y\n--->z";
        assert_eq!(apply_patch("x", spec).unwrap(), "z");
    }

    #[test]
    fn patch_strips_only_newlines_not_spaces() {
        let spec = "@@@\n  a  \n\n--->\n  b  \n";
        assert_eq!(apply_patch("x   a  y", spec).unwrap(), "x   b  y");
    }

    #[test]
    fn patch_missing_anchor_is_an_error_not_a_silent_noop() {
        let spec = "@@@nope\n--->x";
        assert_eq!(
            apply_patch("something else", spec),
            Err(PatchError::AnchorMissing { block: 0 })
        );
    }

    #[test]
    fn patch_reports_which_block_is_missing() {
        let spec = "@@@a\n--->b@@@missing\n--->c";
        assert_eq!(
            apply_patch("a", spec),
            Err(PatchError::AnchorMissing { block: 1 })
        );
    }

    #[test]
    fn patch_without_a_separator_is_malformed() {
        assert_eq!(
            parse_patch("@@@only-one-side"),
            Err(PatchError::Malformed { block: 0 })
        );
    }

    #[test]
    fn patch_with_two_separators_is_malformed() {
        assert_eq!(
            parse_patch("@@@a--->b--->c"),
            Err(PatchError::Malformed { block: 0 })
        );
    }

    #[test]
    fn text_before_the_first_marker_is_ignored() {
        assert!(
            parse_patch("prose about the defect\n@@@a\n--->b")
                .unwrap()
                .len()
                == 1
        );
        assert!(parse_patch("no markers at all").unwrap().is_empty());
    }

    // --- freezing and grafting ---------------------------------------------

    #[test]
    fn freeze_keeps_only_the_test_section() {
        let src = "pub fn f() {}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n";
        assert_eq!(
            freeze_suite(src, "budget"),
            "#[cfg(test)]\nmod frozen {\n    #[test]\n    fn t() {}\n}\n"
        );
    }

    #[test]
    fn freeze_rewrites_super_star_to_the_named_module() {
        let src = "#[cfg(test)]\nmod tests {\n    use super::*;\n}\n";
        let got = freeze_suite(src, "gate");
        assert!(got.contains("use crate::gate::*;"), "{got}");
        assert!(!got.contains("use super::*;"));
    }

    #[test]
    fn freeze_of_a_file_with_no_tests_is_empty() {
        assert_eq!(freeze_suite("pub fn f() {}\n", "m"), "");
    }

    #[test]
    fn graft_renames_modules_and_keeps_indentation() {
        let suite = "#[cfg(test)]\nmod tests {\n    mod helper {}\n}\n";
        assert_eq!(
            rename_grafted_modules(suite),
            "#[cfg(test)]\nmod dfx_tests {\n    mod dfx_helper {}\n}\n"
        );
    }

    #[test]
    fn graft_renames_only_the_first_module_on_a_line() {
        assert_eq!(
            rename_grafted_modules("mod a; mod b;\n"),
            "mod dfx_a; mod b;\n"
        );
    }

    #[test]
    fn graft_leaves_non_module_lines_alone() {
        let suite = "use super::Admission;\n    // mod in a comment\n}\n";
        assert_eq!(rename_grafted_modules(suite), suite);
    }

    #[test]
    fn graft_capture_is_lowercase_only() {
        // `mod Tests` matches with an empty name, so `Tests` is left in place.
        assert_eq!(rename_grafted_modules("mod Tests {\n"), "mod dfx_Tests {\n");
        assert_eq!(
            rename_grafted_modules("mod frozen_2 {\n"),
            "mod dfx_frozen_2 {\n"
        );
    }

    #[test]
    fn module_stem_is_the_target_basename_without_extension() {
        assert_eq!(module_stem("crates/farmerbob-core/src/budget.rs"), "budget");
        assert_eq!(module_stem("lib.rs"), "lib");
    }

    // --- compile-error detection -------------------------------------------

    #[test]
    fn compile_error_needs_a_bracketed_error_code_or_the_message() {
        assert!(is_compile_error("error[E0308]: mismatched types\n"));
        assert!(is_compile_error("error[E0432]: unresolved import\n"));
        assert!(is_compile_error("error: could not compile `x` (lib)\n"));
        assert!(!is_compile_error("error: test failed, to rerun pass\n"));
        assert!(!is_compile_error(" error[E0308]: indented, not anchored\n"));
        assert!(
            !is_compile_error("error[E0308] mismatched types\n"),
            "the colon is required"
        );
        assert!(!is_compile_error("error[Ex]: not a number\n"));
        assert!(!is_compile_error("error[E]: no digits at all\n"));
        assert!(!is_compile_error("warning: unused variable\n"));
    }

    // --- classification -----------------------------------------------------

    #[test]
    fn a_failing_suite_detected_the_defect() {
        let m = classify_run(&CargoRun {
            ok: false,
            killed: false,
            compile_error: false,
            tests_declared: 3,
            graft_lines: 40,
        });
        assert_eq!(m.value().copied(), Some(Detection::Caught));
    }

    #[test]
    fn a_passing_suite_missed_the_defect() {
        let m = classify_run(&CargoRun {
            ok: true,
            killed: false,
            compile_error: false,
            tests_declared: 3,
            graft_lines: 40,
        });
        assert_eq!(m.value().copied(), Some(Detection::Missed));
    }

    #[test]
    fn a_suite_that_did_not_compile_measured_nothing() {
        let m = classify_run(&CargoRun {
            ok: false,
            killed: false,
            compile_error: true,
            tests_declared: 3,
            graft_lines: 40,
        });
        assert!(!m.is_observed(), "nocompile is an absence, never a catch");
        assert_eq!(m.value().copied(), None);
        match m.absent() {
            Some(Absent::InstrumentFailed { reason }) => assert!(!reason.is_empty()),
            other => panic!("expected an instrument failure, got {other:?}"),
        }
    }

    #[test]
    fn a_killed_run_is_never_a_detection() {
        let m = classify_run(&CargoRun {
            ok: false,
            killed: true,
            compile_error: false,
            tests_declared: 3,
            graft_lines: 40,
        });
        assert!(
            !m.is_observed(),
            "a suite that never finished detected nothing"
        );
    }

    #[test]
    fn a_suite_with_no_tests_measured_nothing() {
        let m = classify_run(&CargoRun {
            ok: true,
            killed: false,
            compile_error: false,
            tests_declared: 0,
            graft_lines: 40,
        });
        assert!(!m.is_observed());
        match m.absent() {
            Some(Absent::NothingToMeasure { reason }) => assert!(!reason.is_empty()),
            other => panic!("expected nothing-to-measure, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_graft_measured_nothing() {
        let m = classify_run(&CargoRun {
            ok: true,
            killed: false,
            compile_error: false,
            tests_declared: 0,
            graft_lines: 0,
        });
        assert!(!m.is_observed());
    }

    // --- defect classes ------------------------------------------------------

    #[test]
    fn the_three_defect_classes_are_exclusive() {
        assert_eq!(
            classify_defect(&Measurement::observed(Detection::Caught)),
            DefectClass::Valid
        );
        assert_eq!(
            classify_defect(&Measurement::observed(Detection::Missed)),
            DefectClass::BeyondReference
        );
        let nocompile = Measurement::<Detection>::instrument_failed("did not compile");
        match classify_defect(&nocompile) {
            DefectClass::NotADefect(Absent::InstrumentFailed { reason }) => {
                assert_eq!(reason, "did not compile")
            }
            other => panic!("expected not-a-defect, got {other:?}"),
        }
    }

    #[test]
    fn validation_line_format_is_byte_for_byte() {
        assert_eq!(
            validation_line("budget-3", DefectClass::Valid),
            "  budget-3                     valid (conformance detects it)"
        );
        assert_eq!(
            validation_line("budget-3", DefectClass::BeyondReference),
            "  budget-3                     BEYOND the reference (it misses this one)"
        );
        let nocompile = Measurement::<Detection>::instrument_failed("did not compile");
        assert_eq!(
            validation_line("budget-3", classify_defect(&nocompile)),
            "  budget-3                     excluded (nocompile) -- not a defect"
        );
    }

    // --- aggregation ---------------------------------------------------------

    #[test]
    fn nocompile_and_never_run_leave_every_denominator() {
        let arms = vec!["a".to_string()];
        let valid = vec!["d1".to_string(), "d2".to_string(), "d3".to_string()];
        let beyond: Vec<String> = vec![];
        let table = cells(&[
            ("a", "d1", Detection::Caught),
            ("a", "d2", Detection::Missed),
        ]);
        // d3 has no cell at all: unmeasured, like the script's missing file.
        let rows = compute_rows(&arms, &valid, &beyond, &lookup(&table));
        assert_eq!(rows[0].caught, 1);
        assert_eq!(rows[0].comparable, 2, "d3 must not enter the denominator");
        assert_eq!(rows[0].sensitivity.value().copied(), Some(0.5));
        assert_eq!(rows[0].missed, vec!["d2".to_string()]);
    }

    #[test]
    fn zero_of_zero_is_absent_not_zero() {
        let arms = vec!["a".to_string()];
        let valid = vec!["d1".to_string()];
        let beyond: Vec<String> = vec![];
        let table: Vec<(String, String, Measurement<Detection>)> = vec![(
            "a".to_string(),
            "d1".to_string(),
            Measurement::not_attempted(),
        )];
        let rows = compute_rows(&arms, &valid, &beyond, &lookup(&table));
        assert_eq!(rows[0].comparable, 0);
        assert!(
            rows[0].sensitivity.absent().is_some(),
            "no measurement is not a measured zero"
        );
    }

    #[test]
    fn zero_of_two_is_a_measured_zero() {
        let arms = vec!["a".to_string()];
        let valid = vec!["d1".to_string(), "d2".to_string()];
        let table = cells(&[
            ("a", "d1", Detection::Missed),
            ("a", "d2", Detection::Missed),
        ]);
        let rows = compute_rows(&arms, &valid, &[], &lookup(&table));
        assert_eq!(rows[0].sensitivity.value().copied(), Some(0.0));
    }

    #[test]
    fn beyond_reference_defects_are_counted_separately() {
        let arms = vec!["a".to_string()];
        let valid = vec!["d1".to_string()];
        let beyond = vec!["b1".to_string(), "b2".to_string(), "b3".to_string()];
        let table = cells(&[
            ("a", "d1", Detection::Caught),
            ("a", "b1", Detection::Caught),
            ("a", "b2", Detection::Missed),
        ]);
        let rows = compute_rows(&arms, &valid, &beyond, &lookup(&table));
        assert_eq!(rows[0].caught, 1);
        assert_eq!(rows[0].comparable, 1);
        assert_eq!(rows[0].beyond_caught, 1, "b1: better than the reference");
        assert_eq!(
            rows[0].beyond_of, 2,
            "b3 did not run, so it is not comparable"
        );
        assert_eq!(rows[0].missed, Vec::<String>::new());
    }

    #[test]
    fn only_validated_defects_appear_in_the_missed_list() {
        let arms = vec!["a".to_string()];
        let valid = vec!["d1".to_string()];
        let beyond = vec!["b1".to_string()];
        let table = cells(&[
            ("a", "d1", Detection::Missed),
            ("a", "b1", Detection::Missed),
        ]);
        let rows = compute_rows(&arms, &valid, &beyond, &lookup(&table));
        assert_eq!(
            rows[0].missed,
            vec!["d1".to_string()],
            "the missed list is the validated set only; beyond-ref misses are reported as a count"
        );
        assert_eq!(rows[0].beyond_of, 1);
        assert_eq!(rows[0].beyond_caught, 0);
    }

    #[test]
    fn the_beyond_column_prints_a_dash_when_nothing_there_was_comparable() {
        let arms = vec!["a".to_string()];
        let valid = vec!["d1".to_string()];
        let table = cells(&[("a", "d1", Detection::Caught)]);
        let rows = compute_rows(&arms, &valid, &[], &lookup(&table));
        let got = format_table(&rows, &[]);
        assert!(
            got.lines().nth(2).unwrap().ends_with("           -"),
            "{got}"
        );
    }

    #[test]
    fn freeze_then_graft_produces_a_module_the_reference_cannot_collide_with() {
        let src = "pub fn f() {}\n#[cfg(test)]\nmod tests {\n    use super::*;\n}\n";
        let grafted = rename_grafted_modules(&freeze_suite(src, "budget"));
        assert!(grafted.contains("mod dfx_frozen"), "{grafted}");
        assert!(!grafted.contains("mod tests"), "{grafted}");
        assert!(grafted.contains("use crate::budget::*;"), "{grafted}");
    }

    #[test]
    fn rows_come_back_in_arm_order() {
        let arms = vec!["z".to_string(), "a".to_string()];
        let rows = compute_rows(&arms, &[], &[], &lookup(&[]));
        let names: Vec<&str> = rows.iter().map(|r| r.arm.as_str()).collect();
        assert_eq!(names, vec!["z", "a"]);
    }

    // --- the report ----------------------------------------------------------

    /// The worked example: three suites, four validated defects and two beyond
    /// the reference. Pinned byte-for-byte because other scripts grep this.
    ///
    /// The order is itself part of the pin: `codex-luna` and `sonnet-forty`
    /// each caught one defect the reference misses, so both outrank
    /// `opus-max` despite its perfect sensitivity -- the BEYOND REF column
    /// comes first, which is the whole point of keeping it.
    #[test]
    fn summary_table_format_is_byte_for_byte() {
        let arms = vec![
            "opus-max".to_string(),
            "codex-luna".to_string(),
            "sonnet-forty".to_string(),
        ];
        let valid = vec![
            "d1".to_string(),
            "d2".to_string(),
            "d3".to_string(),
            "d4".to_string(),
        ];
        let beyond = vec!["b1".to_string(), "b2".to_string()];
        let table = cells(&[
            // opus-max: catches every validated defect, misses both beyond.
            ("opus-max", "d1", Detection::Caught),
            ("opus-max", "d2", Detection::Caught),
            ("opus-max", "d3", Detection::Caught),
            ("opus-max", "d4", Detection::Caught),
            ("opus-max", "b1", Detection::Missed),
            ("opus-max", "b2", Detection::Missed),
            // codex-luna: 2 of 3 comparable, and one beyond the reference.
            ("codex-luna", "d1", Detection::Caught),
            ("codex-luna", "d2", Detection::Caught),
            ("codex-luna", "d3", Detection::Missed),
            ("codex-luna", "b1", Detection::Caught),
            ("codex-luna", "b2", Detection::Missed),
            // sonnet-forty: measured nothing at all, but caught b2.
            ("sonnet-forty", "b2", Detection::Caught),
        ]);
        let rows = compute_rows(&arms, &valid, &beyond, &lookup(&table));
        let got = format_table(&rows, &beyond);
        // Written as one literal per line, because a `\`-continued string
        // literal silently drops the leading spaces of its continuation lines,
        // and the footer's indentation is part of the format.
        let want = concat!(
            "\n",
            "SUITE                      CAUGHT   OF  SENSITIVITY  BEYOND REF\n",
            "codex-luna                      2    3          67%         1/2\n",
            "sonnet-forty                    0    0          n/a         1/1\n",
            "opus-max                        4    4         100%         0/2\n",
            "\n",
            "  BEYOND REF counts defects the reference conformance suite does NOT detect.\n",
            "  Catching one proves a suite is strictly better than the reference; it is the\n",
            "  only column here that can separate a field where everyone catches the rest.\n",
        );
        assert_eq!(
            got, want,
            "the table must match the script's python exactly"
        );
    }

    #[test]
    fn the_beyond_reference_footnote_appears_only_when_one_exists() {
        let rows = compute_rows(&["a".to_string()], &[], &[], &lookup(&[]));
        let got = format_table(&rows, &[]);
        assert!(!got.contains("BEYOND REF counts"));
        assert_eq!(
            got.lines().count(),
            3,
            "a blank line, the header, and one row"
        );
    }

    #[test]
    fn unmeasured_sorts_with_a_measured_zero_and_last() {
        // All three have the same beyond count, so the sensitivity decides:
        // 1.0 first, then the measured 0% and the unmeasured n/a tied in arm
        // order -- Python's `sensitivity or -1` treats 0.0 as absent.
        let arms = vec![
            "measured-zero".to_string(),
            "unmeasured".to_string(),
            "perfect".to_string(),
        ];
        let valid = vec!["d1".to_string()];
        let table = cells(&[
            ("measured-zero", "d1", Detection::Missed),
            ("perfect", "d1", Detection::Caught),
        ]);
        let rows = compute_rows(&arms, &valid, &[], &lookup(&table));
        let got = format_table(&rows, &[]);
        let names: Vec<&str> = got.lines().skip(2).map(|l| l[..24].trim_end()).collect();
        assert_eq!(names, vec!["perfect", "measured-zero", "unmeasured"]);
    }

    #[test]
    fn beyond_reference_caught_outranks_sensitivity() {
        let arms = vec!["sharp".to_string(), "lucky".to_string()];
        let valid = vec!["d1".to_string()];
        let beyond = vec!["b1".to_string()];
        let table = cells(&[
            ("sharp", "d1", Detection::Caught),
            ("lucky", "d1", Detection::Missed),
            ("lucky", "b1", Detection::Caught),
            ("sharp", "b1", Detection::Missed),
        ]);
        let rows = compute_rows(&arms, &valid, &beyond, &lookup(&table));
        let got = format_table(&rows, &beyond);
        assert!(got.lines().nth(2).unwrap().starts_with("lucky"), "{got}");
    }

    #[test]
    fn json_report_matches_the_script_shape() {
        let arms = vec!["arm-one".to_string()];
        let valid = vec!["d1".to_string(), "d2".to_string()];
        let table = cells(&[
            ("arm-one", "d1", Detection::Caught),
            ("arm-one", "d2", Detection::Missed),
        ]);
        let rows = compute_rows(&arms, &valid, &[], &lookup(&table));
        let got = report_json(2, 0, &rows).dump(0);
        let want = "{\n \
 \"validated_defects\": 2,\n \
 \"beyond_reference_defects\": 0,\n \
 \"arms\": {\n  \
 \"arm-one\": {\n   \
 \"caught\": 1,\n   \
 \"comparable\": 2,\n   \
 \"sensitivity\": 0.5,\n   \
 \"beyond_reference_caught\": 0,\n   \
 \"beyond_reference_of\": 0,\n   \
 \"missed\": [\n    \
 \"d2\"\n   \
 ]\n  \
 }\n \
 }\n\
}";
        assert_eq!(got, want, "the JSON must match json.dump(indent=1)");
    }

    #[test]
    fn json_report_is_parseable_and_nulls_an_absent_sensitivity() {
        let arms = vec!["a".to_string()];
        let rows = compute_rows(&arms, &[], &[], &lookup(&[]));
        let text = report_json(0, 0, &rows).dump(0);
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["validated_defects"], 0);
        assert!(value["arms"]["a"]["sensitivity"].is_null());
        assert_eq!(value["arms"]["a"]["missed"], serde_json::json!([]));
    }

    // --- orchestration helpers ------------------------------------------------

    #[test]
    fn host_is_the_target_module_not_lib_rs() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("lib.rs"), "").unwrap();
        std::fs::write(src.join("budget.rs"), "").unwrap();
        assert_eq!(
            host_module(&src, "crates/farmerbob-core/src/budget.rs"),
            Some(src.join("budget.rs"))
        );
        assert_eq!(
            host_module(&src, "crates/farmerbob-core/src/missing.rs"),
            Some(src.join("lib.rs"))
        );
        assert_eq!(host_module(src.parent().unwrap(), "x/y.rs"), None);
    }

    #[test]
    fn patches_are_listed_sorted_and_named_without_extension() {
        let dir = TempDir::new().unwrap();
        for n in ["b", "a", "c"] {
            std::fs::write(dir.path().join(format!("{n}.patch")), "").unwrap();
        }
        std::fs::write(dir.path().join("notes.txt"), "").unwrap();
        assert_eq!(list_patches(dir.path()), vec!["a", "b", "c"]);
    }

    #[test]
    fn a_missing_defect_directory_lists_nothing() {
        assert!(list_patches(Path::new("/nonexistent/defects")).is_empty());
    }

    #[test]
    fn candidate_suites_need_a_live_worktree_and_a_test_in_the_frozen_suite() {
        let root = TempDir::new().unwrap();
        let wt = root.path().join("bead--arm-one");
        std::fs::create_dir_all(wt.join("crates/x/src")).unwrap();
        std::fs::write(
            wt.join("crates/x/src/budget.rs"),
            "pub fn f() {}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n",
        )
        .unwrap();
        // A live run: skipped even though it has a suite.
        let live = root.path().join("bead--arm-live");
        std::fs::create_dir_all(live.join("crates/x/src")).unwrap();
        std::fs::write(live.join(".fb-task.md"), "running").unwrap();
        std::fs::write(
            live.join("crates/x/src/budget.rs"),
            "pub fn f() {}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n",
        )
        .unwrap();
        // No tests at all: frozen suite is empty, so the arm is dropped.
        let notests = root.path().join("bead--arm-bare");
        std::fs::create_dir_all(notests.join("crates/x/src")).unwrap();
        std::fs::write(notests.join("crates/x/src/budget.rs"), "pub fn f() {}\n").unwrap();

        let tmp = TempDir::new().unwrap();
        let arms = discover_suites(root.path(), "bead", "crates/x/src/budget.rs", tmp.path());
        assert_eq!(arms, vec!["arm-one".to_string()]);
        let frozen = std::fs::read_to_string(tmp.path().join("s.arm-one.rs")).unwrap();
        assert!(frozen.contains("mod frozen"), "{frozen}");
        assert!(frozen.contains("use crate::budget::*;") || !frozen.contains("use super::*;"));
    }

    #[test]
    fn a_reference_without_crates_cannot_be_copied() {
        let repo = TempDir::new().unwrap();
        let dest = TempDir::new().unwrap();
        assert!(copy_reference(repo.path(), &dest.path().join("ref")).is_err());
    }

    #[test]
    fn preparing_a_cell_replaces_whatever_was_there() {
        let repo = TempDir::new().unwrap();
        std::fs::create_dir_all(repo.path().join("crates/x/src")).unwrap();
        std::fs::write(repo.path().join("crates/x/src/lib.rs"), "clean\n").unwrap();
        let tmp = TempDir::new().unwrap();
        let reference = tmp.path().join("ref");
        copy_reference(repo.path(), &reference).unwrap();

        let cell = tmp.path().join("r.a.d1");
        prepare(&reference, &cell).unwrap();
        let f = cell.join("crates/x/src/lib.rs");
        std::fs::write(&f, "dirty\n").unwrap();
        prepare(&reference, &cell).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "clean\n");
    }
}


