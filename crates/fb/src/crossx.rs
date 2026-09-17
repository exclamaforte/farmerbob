//! Cross-examination: run every candidate's test suite against every candidate's
//! implementation, N² local executions, zero tokens, no model opinions.
//!
//! This is a behaviour-for-behaviour port of `fb-crossx.sh`. The shell harness
//! conflated "measured zero" with "not measured" in several places — a missing
//! score file became "all arms failed the gate", a suite that could not
//! compile became a silent `0` in a count, and a survival fraction with no
//! comparable suites became `0.0`. Every such spot is made unrepresentable here
//! with [`farmerbob_core::measurement::Measurement`]: a quantity is either
//! [`farmerbob_core::measurement::Measurement::Observed`] or it carries a stated
//! reason it is [`farmerbob_core::measurement::Measurement::Missing`].
//!
//! The public surface is [`run_cmd`]; everything below it is split into a pure,
//! input-independent core (gate filtering, the diagonal invariant, the partition
//! shape, survival/discovery, and the two printed tables) and an orchestration
//! layer that does the filesystem and `cargo` work and feeds the core.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use farmerbob_core::crossx::{Cell, Matrix};
use farmerbob_core::measurement::Measurement;
use serde::Serialize;

/// Default number of `cargo test` cycles run concurrently.
const CX_SLOTS_DEFAULT: usize = 4;

/// Per-cell timeout for a single `cargo test` run, in seconds.
const CARGO_TIMEOUT_SECS: u64 = 300;

/// `$HOME`, required for every path the script builds from it.
fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| String::from("/home/gabe")))
}

/// The worktree root: `$HOME/.local/share/farmerbob/worktrees`.
fn wt_root() -> PathBuf {
    crate::paths::worktrees()
}

/// The repo root: hardcoded to match the script's `$REPO`.
fn repo_root() -> PathBuf {
    PathBuf::from("/home/gabe/Documents/farmerbob")
}

/// `$HOME/.local/share/farmerbob/logs`.
fn logs_dir() -> PathBuf {
    crate::paths::logs()
}

/// Public entry point, mirroring the script's positional parameters in order:
/// `bead` (`$1`), `krate` (`$2`, default `farmerbob-core`), `file` (`$3`).
///
/// Returns the process exit code exactly as the script does: `0` on a complete,
/// fully-passing matrix written to `<logs>/<bead>.crossx.json`; `1` on a usage
/// error (fewer than two candidates); `3` when the diagonal invariant fails, in
/// which case scores are deliberately NOT written because a broken transplant
/// must say so rather than emit a confident null.
pub fn run_cmd(bead: &str, krate: &str, file: &str) -> i32 {
    match run(bead, krate, file) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

/// The real body of [`run_cmd`], returning its exit code or a structured error.
fn run(bead: &str, krate: &str, file: &str) -> Result<i32> {
    let root = wt_root();
    let tmp = TempDir::new().context("creating temporary directory")?;

    // 1. Discover candidate worktrees: every `$WT_ROOT/$BEAD--*`.
    let discovered = discover_arms(&root, bead);

    // 2. Only arms that PASSED the gate are cross-examined. A missing or
    //    unparseable score file is NOT "everyone failed" — it is "we could not
    //    measure the gate", and the script's behaviour in that case is to skip
    //    the filter entirely (an empty `PASSED` passes everyone). That absence
    //    is carried as `Measurement::Missing` and resolved to "include all".
    let passed = load_passed(bead);

    let mut arms: Vec<String> = Vec::new();
    for a in &discovered {
        if arm_passes_gate(&passed, a) {
            arms.push(a.clone());
        } else {
            println!("  skip {a} (did not pass the gate)");
        }
    }

    // Printed before the <2 check, exactly like the script.
    println!("candidates: {}", arms.join(" "));
    if arms.len() < 2 {
        println!("need >=2");
        return Ok(1);
    }

    let srcdir = format!("crates/{krate}/src");

    // 3. Collect each arm's test sources into graft files.
    for a in &arms {
        let (lines, tests, files) = collect_suites(
            &root,
            &repo_root(),
            bead,
            krate,
            file,
            a,
            tmp.path(),
            &srcdir,
        )
        .with_context(|| format!("collecting suites for {a}"))?;
        println!("  suite {a:<22} {lines} lines, {tests} tests, {files} file(s)");
    }

    // 4. N² cargo cycles, bounded by `FB_CX_SLOTS`.
    let slots: usize = std::env::var("FB_CX_SLOTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(CX_SLOTS_DEFAULT)
        .max(1);
    let results = run_matrix(&root, bead, krate, &srcdir, tmp.path(), &arms, slots)?;

    // Build the square matrix of cells, impl × suite.
    let cells = build_matrix(&arms, &results)?;

    // 5. THE DIAGONAL INVARIANT. Every arm's own suite must pass against its own
    //    implementation; a failure is the transplant, never the candidate.
    let diag_bad = diagonal_bad(&arms, &cells);
    if !diag_bad.is_empty() {
        println!();
        println!(
            "VOID: the diagonal is not all-pass -- {}",
            diag_bad.join(" ")
        );
        println!("A suite that cannot run against the code it shipped with is a grafting failure.");
        println!("Scores are NOT written; fix the transplant before trusting any cell.");
        return Ok(3);
    }

    // 6. THE PARTITION SHAPE. Self-consistent camps that pass within and fail
    //    across are neither a defect (1-of-N) nor an over-fitted suite (N-1-of-N).
    if let Some(part) = detect_partition(&arms, &cells) {
        println!();
        println!("SPEC-AMBIGUOUS: the field partitions into self-consistent camps -- {part}");
        println!(
            "Each camp passes within itself and fails across, which is neither a defect (1-of-N)"
        );
        println!("nor an over-fitted suite (N-1-of-N). Two readings of the spec, both implemented");
        println!("correctly. Treat every cross-camp failure as vetoed and fix the specification.");
    }

    // 7. The matrix table.
    print!("{}", format_matrix(&arms, &cells));

    // 8. Survival/discovery per arm, the JSON, and the summary table.
    let stats = compute_stats(&arms, &cells);
    let out = logs_dir().join(format!("{bead}.crossx.json"));
    write_json(&out, &stats).with_context(|| format!("writing {}", out.display()))?;
    print!("{}", format_summary(&arms, &stats));
    println!("-> {}", out.display());

    Ok(0)
}

// ---------------------------------------------------------------------------
// Pure core
// ---------------------------------------------------------------------------

/// Decide whether an arm survives the gate filter.
///
/// The script's `PASSED` is built by a python snippet guarded with
/// `2>/dev/null`: any failure leaves it empty, and an empty `PASSED` skips the
/// filter so every arm is cross-examined. So an *absent* measurement behaves
/// exactly like an empty observed set — both include every arm. Only a
/// non-empty observed set excludes arms not named in it.
pub fn arm_passes_gate(passed: &Measurement<BTreeSet<String>>, arm: &str) -> bool {
    match passed.value() {
        // Absent measurement: we could not read the gate. Mirror the script's
        // empty-`PASSED` branch and include the arm.
        None => true,
        // Observed but empty: no arm passed the gate we could see; the script's
        // `if [ -n "$PASSED" ]` is false, so again include everyone.
        Some(set) if set.is_empty() => true,
        // Observed and non-empty: include only named arms.
        Some(set) => set.contains(arm),
    }
}

/// The diagonal invariant: every arm whose own suite did not pass against its
/// own implementation. Returns the labels `arm(outcome)` for each failure, in
/// arm order. An empty vec means the matrix is sound.
pub fn diagonal_bad(arms: &[String], cells: &Matrix) -> Vec<String> {
    let mut bad = Vec::new();
    for a in arms {
        match cells.get(a, a) {
            Some(Cell::Pass) => {}
            other => {
                let tag = match other {
                    Some(Cell::Fail) => "fail",
                    _ => "nocompile",
                };
                bad.push(format!("{a}({tag})"));
            }
        }
    }
    bad
}

/// Detect the partition shape: arms split into self-consistent camps that pass
/// within themselves and fail across. Returns the joined camp description
/// (`{a,b} | {c,d}`) when the field partitions, or `None` when it does not
/// (too few arms, no clean split, or the split is not internally consistent).
///
/// Mirrors the script's `detect_partition`: `camp(a)` is the set of suites `a`'s
/// implementation passes, arms are grouped by equal camp, and a valid partition
/// requires at least two camps, every member inside its camp, and every
/// cross-camp cell a failure.
pub fn detect_partition(arms: &[String], cells: &Matrix) -> Option<String> {
    if arms.len() < 4 {
        return None;
    }
    let mut camps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for a in arms {
        let mut camp = BTreeSet::new();
        for s in arms {
            if cells.get(a, s) == Some(Cell::Pass) {
                camp.insert(s.clone());
            }
        }
        camps.insert(a.clone(), camp);
    }

    // Group arms by equal camp, preserving first-appearance order (the script's
    // dict preserves insertion order).
    let mut groups: Vec<(BTreeSet<String>, Vec<String>)> = Vec::new();
    for (a, c) in &camps {
        if let Some(pos) = groups.iter().position(|(k, _)| k == c) {
            groups[pos].1.push(a.clone());
        } else {
            groups.push((c.clone(), vec![a.clone()]));
        }
    }
    if groups.len() < 2 {
        return None;
    }

    for (camp, members) in &groups {
        // Every member must be inside its own camp.
        if members.iter().any(|m| !camp.contains(m)) {
            return None;
        }
        // Every member must fail every arm outside the camp.
        for m in members {
            for other in arms {
                if camp.contains(other) {
                    continue;
                }
                if cells.get(m, other) != Some(Cell::Fail) {
                    return None;
                }
            }
        }
    }

    let parts: Vec<String> = groups
        .iter()
        .map(|(_, members)| {
            let mut sorted = members.clone();
            sorted.sort();
            format!("{{{}}}", sorted.join(","))
        })
        .collect();
    Some(parts.join(" | "))
}

/// One arm's cross-examination result: everything the summary table and the
/// JSON report need for a single arm.
///
/// `survival` is a [`Measurement`] rather than a bare `f64`: when no
/// discriminating foreign suite ran against this implementation there is no
/// fraction to compute, and reporting `0.0` would be a lie that the script told
/// (its `survival = None` became `"n/a"`). Here the absence is the type.
pub struct ArmRow {
    /// The arm this row describes.
    pub arm: String,
    /// Fraction of OTHER arms' discriminating suites this implementation passes,
    /// or `Missing` when none ran (never a fabricated `0.0`).
    pub survival: Measurement<f64>,
    /// Number of discriminating foreign suites this implementation passed.
    pub survived: usize,
    /// Number of discriminating foreign suites this implementation was measured against.
    pub of: usize,
    /// Number of other implementations this arm's suite broke (0 when over-fitted).
    pub discovery: usize,
    /// Other implementations this arm's suite failed against.
    pub broke: Vec<String>,
    /// Other implementations this arm's suite could not compile against (API mismatch).
    pub api_incompatible_with: usize,
    /// True when this arm's suite discriminates (fails some but not all it compiles against).
    pub suite_discriminating: bool,
    /// True when this arm's suite is over-fitted (breaks essentially every implementation).
    pub suite_overfitted: bool,
}

/// Compute per-arm survival and discovery, mirroring the script's inner python.
///
/// `survival` is the fraction of OTHER arms' *discriminating* suites this
/// implementation passes; an implementation with none is [`Measurement::Missing`]
/// (`NothingToMeasure`), never `0.0`. `discovery` is the number of other
/// implementations this arm's suite breaks, discounted to `0` when the suite is
/// over-fitted (it breaks essentially every implementation it compiles against).
/// `nocompile` is an API mismatch, excluded from every denominator.
pub fn compute_stats(arms: &[String], cells: &Matrix) -> Vec<ArmRow> {
    let cell = |imp: &str, suite: &str| -> Option<Cell> { cells.get(imp, suite) };

    // `compiles(s)` = implementations (other than `s`) this suite built against.
    let compiles = |s: &str| -> Vec<String> {
        arms.iter()
            .filter(|i| *i != s && cell(i, s) != Some(Cell::Error))
            .cloned()
            .collect()
    };

    // A suite discriminates when, among the impls it compiles against, it fails
    // some but not all.
    let mut disc: Vec<String> = Vec::new();
    for s in arms {
        let c = compiles(s);
        if c.len() < 2 {
            continue;
        }
        let f = c.iter().filter(|i| cell(i, s) == Some(Cell::Fail)).count();
        if 0 < f && f < c.len() {
            disc.push(s.clone());
        }
    }

    let mut rows = Vec::new();
    for a in arms {
        let others: Vec<&String> = arms.iter().filter(|o| *o != a).collect();

        let rel: Vec<String> = disc
            .iter()
            .filter(|s| *s != a && cell(a, s) != Some(Cell::Error))
            .cloned()
            .collect();
        let survived = rel
            .iter()
            .filter(|s| cell(a, s) == Some(Cell::Pass))
            .count();
        let survival = if rel.is_empty() {
            Measurement::nothing_to_measure(
                "no discriminating foreign suite ran against this implementation",
            )
        } else {
            Measurement::observed(survived as f64 / rel.len() as f64)
        };

        let broke: Vec<String> = others
            .iter()
            .filter(|i| cell(i, a) == Some(Cell::Fail))
            .map(|i| (*i).clone())
            .collect();
        let incompat: Vec<String> = others
            .iter()
            .filter(|i| cell(i, a) == Some(Cell::Error))
            .map(|i| (*i).clone())
            .collect();

        let ca = compiles(a);
        let overfit = !ca.is_empty() && broke.len() == ca.len();

        rows.push(ArmRow {
            arm: a.clone(),
            survival,
            survived,
            of: rel.len(),
            discovery: if overfit { 0 } else { broke.len() },
            broke,
            api_incompatible_with: incompat.len(),
            suite_discriminating: disc.contains(a),
            suite_overfitted: overfit,
        });
    }
    rows
}

/// Render the impl×suite matrix table, byte-for-byte with the script's `printf`.
///
/// Header is `impl \ suite` left-justified to 24, then each suite name truncated
/// to 9 chars left-justified to 10. Each row is the arm name truncated to 23
/// chars left-justified to 24, then per cell ` .` (pass), ` FAIL` (fail) or
/// ` nocomp` (nocompile), each left-justified to 10.
pub fn format_matrix(arms: &[String], cells: &Matrix) -> String {
    let mut out = String::new();
    out.push_str(&format!("{:<24}", "impl \\ suite"));
    for s in arms {
        out.push_str(&format!("{:<10}", truncate(s, 9)));
    }
    out.push('\n');

    for imp in arms {
        out.push_str(&format!("{:<24}", truncate(imp, 23)));
        for s in arms {
            let tag = match cells.get(imp, s) {
                Some(Cell::Pass) => " .",
                Some(Cell::Fail) => " FAIL",
                _ => " nocomp",
            };
            out.push_str(&format!("{:<10}", tag));
        }
        out.push('\n');
    }
    out
}

/// Render the per-arm summary table, byte-for-byte with the script's python
/// `print(f"{'ARM':<24}{'SURVIVAL':>10}{'DISCOVERY':>11}{'API-INCOMPAT':>14}  SUITE")`.
///
/// Rows are sorted by survival descending (absent ⇒ 0) then discovery descending,
/// matching the script's `sorted(..., key=lambda kv: (-(survival or 0), -discovery))`.
pub fn format_summary(_arms: &[String], stats: &[ArmRow]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{:<24}{:>10}{:>11}{:>14}  SUITE",
        "ARM", "SURVIVAL", "DISCOVERY", "API-INCOMPAT"
    ));
    out.push('\n');

    let mut ordered: Vec<&ArmRow> = stats.iter().collect();
    ordered.sort_by(|x, y| {
        let sx = x.survival.value().copied().unwrap_or(0.0);
        let sy = y.survival.value().copied().unwrap_or(0.0);
        sy.partial_cmp(&sx)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(y.discovery.cmp(&x.discovery))
    });

    for r in ordered {
        let s = if r.of == 0 {
            "n/a".to_string()
        } else {
            format!("{}/{}", r.survived, r.of)
        };
        let tag = if r.suite_overfitted {
            "OVER-FITTED"
        } else if r.suite_discriminating {
            "discriminating"
        } else {
            "no signal"
        };
        out.push_str(&format!(
            "{:<24}{:>10}{:>11}{:>14}  {}",
            r.arm, s, r.discovery, r.api_incompatible_with, tag
        ));
        out.push('\n');
    }
    out
}

/// Serialise per-arm stats to the script's JSON shape, with `indent=1` to match
/// Python's `json.dump(..., indent=1)`. `survival` is `null` when absent.
fn write_json(path: &Path, stats: &[ArmRow]) -> Result<()> {
    let mut root = serde_json::Map::new();
    for r in stats {
        let mut m = serde_json::Map::new();
        m.insert(
            "survival".to_string(),
            serde_json::Value::from(r.survival.value().copied()),
        );
        m.insert("survived".to_string(), serde_json::json!(r.survived));
        m.insert("of".to_string(), serde_json::json!(r.of));
        m.insert("discovery".to_string(), serde_json::json!(r.discovery));
        m.insert("broke".to_string(), serde_json::json!(r.broke));
        m.insert(
            "api_incompatible_with".to_string(),
            serde_json::json!(r.api_incompatible_with),
        );
        m.insert(
            "suite_discriminating".to_string(),
            serde_json::json!(r.suite_discriminating),
        );
        m.insert(
            "suite_overfitted".to_string(),
            serde_json::json!(r.suite_overfitted),
        );
        root.insert(r.arm.clone(), serde_json::Value::Object(m));
    }

    let mut buf = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(
        &mut buf,
        serde_json::ser::PrettyFormatter::with_indent(b" "),
    );
    root.serialize(&mut ser)
        .context("serialising cross-examination results")?;
    std::fs::write(path, buf).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Build the square [`Matrix`] of cells from the indexed results.
fn build_matrix(arms: &[String], results: &BTreeMap<(String, String), Cell>) -> Result<Matrix> {
    let mut cells = Vec::new();
    for imp in arms {
        for s in arms {
            let cell = results
                .get(&(imp.clone(), s.clone()))
                .copied()
                .unwrap_or(Cell::Error);
            cells.push(cell);
        }
    }
    Matrix::new(arms.to_vec(), arms.to_vec(), cells)
        .map_err(|e| anyhow::anyhow!("assembling cross-examination matrix: {e}"))
}

// ---------------------------------------------------------------------------
// Orchestration: filesystem and cargo
// ---------------------------------------------------------------------------

/// Discover candidate worktrees: every directory `$WT_ROOT/$BEAD--*`.
fn discover_arms(root: &Path, bead: &str) -> Vec<String> {
    let prefix = format!("{bead}--");
    let mut arms = Vec::new();
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return arms,
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(&prefix) {
            continue;
        }
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join(".fb-task.md").is_file() {
            continue; // never score a live run
        }
        arms.push(name[prefix.len()..].to_string());
    }
    arms.sort();
    arms
}

/// Read which arms passed the gate, from `<logs>/<bead>.score.json`.
///
/// Any failure to read or parse the file is an *absent* measurement, not
/// "everyone failed": the script's python is guarded so a missing file yields an
/// empty `PASSED` and the filter is skipped. See [`arm_passes_gate`].
fn load_passed(bead: &str) -> Measurement<BTreeSet<String>> {
    let path = logs_dir().join(format!("{bead}.score.json"));
    let txt = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            return Measurement::instrument_failed(&format!(
                "score file {} unreadable: {e}",
                path.display()
            ));
        }
    };
    let value: serde_json::Value = match serde_json::from_str(&txt) {
        Ok(v) => v,
        Err(e) => return Measurement::instrument_failed(&format!("score file invalid JSON: {e}")),
    };
    let arr = match value.as_array() {
        Some(a) => a,
        None => return Measurement::instrument_failed("score file is not a JSON array"),
    };
    let mut set = BTreeSet::new();
    for row in arr {
        #[allow(clippy::collapsible_if)]
        if row.get("verdict").and_then(|v| v.as_str()) == Some("PASS") {
            if let Some(s) = row.get("source").and_then(|v| v.as_str()) {
                set.insert(s.to_string());
            }
        }
    }
    Measurement::observed(set)
}

/// Collect one arm's test sources into graft files under `tmp`, and report the
/// suite's line/test/file counts. Returns `(lines, tests, files)`.
///
/// Filenames come from, in order: the task's own `fb:creates`/`fb:modifies`
/// declaration (precise, survives merge), else the worktree diff, else the
/// single target file. Each `.rs` file's non-test `use` lines are carried into
/// the graft so only genuine signature divergence shows up as `nocompile`, and
/// duplicate imports are dropped so a suite still compiles against its own code.
#[allow(clippy::too_many_arguments)]
fn collect_suites(
    root: &Path,
    repo: &Path,
    bead: &str,
    _krate: &str,
    file: &str,
    arm: &str,
    tmp: &Path,
    srcdir: &str,
) -> Result<(usize, usize, usize)> {
    let wt = root.join(format!("{bead}--{arm}"));

    let declared = read_declared(repo, bead);
    let changed = read_changed(&wt, srcdir);
    let files: Vec<String> = if !declared.is_empty() {
        declared
    } else if !changed.is_empty() {
        changed
    } else {
        vec![file.to_string()]
    };

    let manifest_path = tmp.join(format!("manifest.{arm}"));
    let mut n = 0usize;
    let mut total_lines = 0usize;
    let mut total_tests = 0usize;
    let mut file_count = 0usize;

    for rel in &files {
        let f = wt.join(rel);
        if !f.is_file() {
            continue;
        }
        if f.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let src = std::fs::read_to_string(&f)
            .with_context(|| format!("reading candidate source {}", f.display()))?;

        let uses = top_level_uses(&src);
        let body = test_body(&src, n);
        let uses = prune_uses(uses, &body);
        let assembled = assemble_suite(&uses, &body);

        let slug = slugify(rel);
        let out_path = tmp.join(format!("suite.{arm}.{slug}.rs"));
        std::fs::write(&out_path, &assembled)
            .with_context(|| format!("writing graft {}", out_path.display()))?;

        if !manifest_contains(&manifest_path, rel)? {
            let mut mf = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&manifest_path)
                .with_context(|| format!("opening manifest {}", manifest_path.display()))?;
            use std::io::Write;
            writeln!(mf, "{rel}").ok();
        }

        total_lines += assembled.lines().count();
        total_tests += assembled.lines().filter(|l| l.contains("#[test]")).count();
        file_count += 1;
        n += 1;
    }

    Ok((total_lines, total_tests, file_count))
}

/// Read `fb:creates`/`fb:modifies` file declarations from the task prompt.
fn read_declared(repo: &Path, bead: &str) -> Vec<String> {
    let path = repo.join(".fb/prompts").join(format!("{bead}.md"));
    let Ok(txt) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    parse_declared(&txt)
}

/// Parse `fb:creates`/`fb:modifies` declarations out of prompt markdown.
fn parse_declared(txt: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in txt.lines() {
        let Some(rest) = line.trim().strip_prefix("<!--") else {
            continue;
        };
        let rest = rest.trim_start();
        if !(rest.starts_with("fb:creates") || rest.starts_with("fb:modifies")) {
            continue;
        }
        let after = rest["fb:".len()..].trim_start();
        let kind_end = after
            .find(char::is_whitespace)
            .map(|i| i + 1)
            .unwrap_or(after.len());
        let path_part = after[kind_end..].trim();
        if let Some(path) = path_part.strip_suffix("-->") {
            let p = path.trim().to_string();
            if !p.is_empty() && !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// Files the arm changed, from `git diff --name-only HEAD` and untracked files.
/// A git failure yields an empty list (the script swallows both with
/// `2>/dev/null`), falling through to the target file.
fn read_changed(wt: &Path, srcdir: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    let wt_s = wt.to_string_lossy().into_owned();
    let diff = Command::new("git")
        .args(["-C", &wt_s, "diff", "--name-only", "HEAD", "--", srcdir])
        .output();
    if let Ok(o) = diff {
        for line in String::from_utf8_lossy(&o.stdout).lines() {
            let l = line.trim();
            if !l.is_empty() {
                out.insert(l.to_string());
            }
        }
    }
    let untracked = Command::new("git")
        .args([
            "-C",
            &wt_s,
            "ls-files",
            "--others",
            "--exclude-standard",
            srcdir,
        ])
        .output();
    if let Ok(o) = untracked {
        for line in String::from_utf8_lossy(&o.stdout).lines() {
            let l = line.trim();
            if !l.is_empty() {
                out.insert(l.to_string());
            }
        }
    }
    out.into_iter().collect()
}

/// Run the full N² matrix, returning `impl × suite -> Cell`.
fn run_matrix(
    root: &Path,
    bead: &str,
    krate: &str,
    srcdir: &str,
    tmp: &Path,
    arms: &[String],
    slots: usize,
) -> Result<BTreeMap<(String, String), Cell>> {
    let tasks: Vec<(String, String)> = arms
        .iter()
        .flat_map(|imp| arms.iter().map(move |suite| (imp.clone(), suite.clone())))
        .collect();

    let results: std::sync::Arc<std::sync::Mutex<BTreeMap<(String, String), Cell>>> =
        std::sync::Arc::new(std::sync::Mutex::new(BTreeMap::new()));
    let queue: std::sync::Arc<std::sync::Mutex<std::vec::IntoIter<(String, String)>>> =
        std::sync::Arc::new(std::sync::Mutex::new(tasks.into_iter()));

    let runners = slots.min(arms.len().saturating_mul(arms.len()).max(1));
    let mut handles = Vec::new();
    for _ in 0..runners {
        let results = std::sync::Arc::clone(&results);
        let queue = std::sync::Arc::clone(&queue);
        let root = root.to_path_buf();
        let bead = bead.to_string();
        let krate = krate.to_string();
        let srcdir = srcdir.to_string();
        let tmp = tmp.to_path_buf();
        let handle = std::thread::Builder::new()
            .name("crossx-worker".to_string())
            .spawn(move || {
                loop {
                    let task = match queue.lock() {
                        Ok(mut g) => g.next(),
                        Err(_) => break,
                    };
                    let Some((imp, suite)) = task else { break };
                    let cell = run_one(&root, &bead, &krate, &srcdir, &tmp, &imp, &suite);
                    if let Ok(mut r) = results.lock() {
                        r.insert((imp, suite), cell);
                    }
                }
            })
            .context("spawning cross-examination worker")?;
        handles.push(handle);
    }
    for h in handles {
        let _ = h.join();
    }

    let locked = std::sync::Arc::try_unwrap(results)
        .map_err(|_| anyhow::anyhow!("results arc still shared"))?;
    let results_map = locked
        .into_inner()
        .map_err(|_| anyhow::anyhow!("results map poisoned"))?;
    Ok(results_map)
}

/// Run one `impl`'s copy with `suite`'s tests grafted in, returning the cell.
fn run_one(
    root: &Path,
    bead: &str,
    krate: &str,
    srcdir: &str,
    tmp: &Path,
    imp: &str,
    suite: &str,
) -> Cell {
    let dest = tmp.join(format!("run.{imp}.{suite}"));
    let _ = std::fs::remove_dir_all(&dest);
    if std::fs::create_dir_all(&dest).is_err() {
        return Cell::Error;
    }

    let wt = root.join(format!("{bead}--{imp}"));
    if copy_worktree(&wt, &dest).is_err() {
        let _ = std::fs::remove_dir_all(&dest);
        return Cell::Error;
    }

    // Strip every test module from the copied sources: keep lines before the
    // first `#[cfg(test)]`.
    let src_path = dest.join(srcdir);
    if strip_test_modules(&src_path).is_err() {
        let _ = std::fs::remove_dir_all(&dest);
        return Cell::Error;
    }

    // Graft `suite`'s collected tests into the modules they belong to.
    let manifest = tmp.join(format!("manifest.{suite}"));
    let rels = match read_manifest_lines(&manifest) {
        Ok(r) => r,
        Err(_) => {
            let _ = std::fs::remove_dir_all(&dest);
            return Cell::Error;
        }
    };
    let mut grafted = 0usize;
    for rel in &rels {
        let sfile = tmp.join(format!("suite.{suite}.{}.rs", slugify(rel)));
        if !sfile.is_file() {
            continue;
        }
        let target = dest.join(rel);
        if target.is_file() {
            #[allow(clippy::collapsible_if)]
            if let (Ok(suite_src), Ok(existing)) = (
                std::fs::read_to_string(&sfile),
                std::fs::read_to_string(&target),
            ) {
                let mut new_contents = existing;
                new_contents.push('\n');
                new_contents.push_str(&suite_src);
                let _ = std::fs::write(&target, new_contents);
                grafted += 1;
            }
        }
    }

    // The implementation has no file this suite tests: a genuine API divergence.
    if grafted == 0 {
        let _ = std::fs::remove_dir_all(&dest);
        return Cell::Error;
    }

    let (ok, output) = match run_cargo_test(&dest, krate, Duration::from_secs(CARGO_TIMEOUT_SECS)) {
        Ok(pair) => pair,
        Err(_) => {
            let _ = std::fs::remove_dir_all(&dest);
            return Cell::Error;
        }
    };
    let _ = std::fs::remove_dir_all(&dest);

    match ok {
        true => Cell::Pass,
        false => {
            if is_compile_error(&output) {
                Cell::Error
            } else {
                Cell::Fail
            }
        }
    }
}

/// Copy `Cargo.toml`, `Cargo.lock`, `rustfmt.toml` and `crates/` into `dest`.
fn copy_worktree(wt: &Path, dest: &Path) -> std::io::Result<()> {
    for name in ["Cargo.toml", "Cargo.lock", "rustfmt.toml"] {
        let s = wt.join(name);
        if s.exists() {
            std::fs::copy(&s, dest.join(name))?;
        }
    }
    let crates = wt.join("crates");
    if crates.is_dir() {
        copy_dir(&crates, &dest.join("crates"))?;
    }
    Ok(())
}

/// Recursively copy a directory tree.
fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &target)?;
        } else {
            std::fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

/// Remove every `#[cfg(test)]` module from `.rs` files under `dir`: keep only
/// lines before the first `#[cfg(test)]`.
fn strip_test_modules(dir: &Path) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            strip_test_modules(&path)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            let src = std::fs::read_to_string(&path)?;
            let kept: String = src
                .lines()
                .take_while(|l| !l.contains("#[cfg(test)]"))
                .collect::<Vec<_>>()
                .join("\n");
            let mut out = kept;
            out.push('\n');
            std::fs::write(&path, out)?;
        }
    }
    Ok(())
}

/// Run `cargo test -p <krate>` in `dir`, killing the process group on timeout.
fn run_cargo_test(dir: &Path, krate: &str, timeout: Duration) -> std::io::Result<(bool, String)> {
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

    // The killer must be CANCELLABLE. It used to `sleep(timeout)` unconditionally and then
    // be `join()`ed, so every cell blocked for the whole timeout no matter how fast the child
    // actually was -- 300 seconds each, sixteen cells, eighty minutes to do work that takes
    // two. It also fired `kill -9` on a process group after the child had exited, which is a
    // signal aimed at whatever now owns that group.
    //
    // The symptom was a process at zero CPU with nine threads parked in futex_do_wait and no
    // cargo running: not a deadlock, a scheduled wait nobody could cancel. Neither the
    // worktree score, nor building it on merge, nor its eighteen passing unit tests caught
    // it. Running it on real input did, in the first minute.
    let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = std::sync::Arc::clone(&finished);
    let killer = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if flag.load(std::sync::atomic::Ordering::Relaxed) {
                return; // the child is already reaped; do not signal anything
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
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
    let combined = {
        let mut s = String::from_utf8_lossy(&output.stdout).into_owned();
        s.push_str(&String::from_utf8_lossy(&output.stderr));
        s
    };
    Ok((output.status.success(), combined))
}

/// True when `output` shows a compile failure: `could not compile` or a line
/// beginning `error[E<digits>:`.
fn is_compile_error(out: &str) -> bool {
    if out.contains("could not compile") {
        return true;
    }
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("error[E") {
            let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
            if digits > 0 && rest.chars().nth(digits) == Some(':') {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Graft text manipulation
// ---------------------------------------------------------------------------

/// Top-level `use` lines that appear before the first `#[cfg(test)]`.
fn top_level_uses(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in src.lines() {
        if line.contains("#[cfg(test)]") {
            break;
        }
        if line.trim_start().starts_with("use ") {
            out.push(line.to_string());
        }
    }
    out
}

/// The `#[cfg(test)]` section of `src`, with each `mod tests` renamed to
/// `mod xtests_<n>` so it does not collide with any existing module.
fn test_body(src: &str, idx: usize) -> String {
    let mut in_test = false;
    let mut out = String::new();
    for line in src.lines() {
        if line.contains("#[cfg(test)]") {
            in_test = true;
        }
        if !in_test {
            continue;
        }
        if let Some(rest) = line.trim_start().strip_prefix("mod tests") {
            let indent_len = line.len() - line.trim_start().len();
            let indent = &line[..indent_len];
            out.push_str(indent);
            out.push_str("mod xtests_");
            out.push_str(&idx.to_string());
            out.push_str(rest);
            out.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Drop a carried `use` when the body already declares it (a duplicate explicit
/// import would be E0252, a hard error that voids the matrix).
fn prune_uses(uses: Vec<String>, body: &str) -> Vec<String> {
    uses.into_iter()
        .filter(|u| {
            let t = u.trim();
            !t.is_empty() && !body.contains(t)
        })
        .collect()
}

/// Assemble a graft file: the test body, with the carried `use` lines injected
/// just before the first `mod xtests_<n> {`.
fn assemble_suite(uses: &[String], body: &str) -> String {
    let mut out = String::new();
    let mut inserted = false;
    for line in body.lines() {
        out.push_str(line);
        out.push('\n');
        if !inserted && line.trim_start().starts_with("mod xtests_") {
            for u in uses {
                out.push_str(u);
                out.push('\n');
            }
            inserted = true;
        }
    }
    out
}

/// Slugify a path the way the script's `tr -c 'a-zA-Z0-9' '_'` does.
fn slugify(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Whether `manifest` already lists `rel`.
fn manifest_contains(manifest: &Path, rel: &str) -> std::io::Result<bool> {
    if !manifest.exists() {
        return Ok(false);
    }
    for line in std::fs::read_to_string(manifest)?.lines() {
        if line.trim() == rel {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Read the rel-path lines of a manifest file.
fn read_manifest_lines(manifest: &Path) -> std::io::Result<Vec<String>> {
    let mut out = Vec::new();
    if !manifest.exists() {
        return Ok(out);
    }
    for line in std::fs::read_to_string(manifest)?.lines() {
        let l = line.trim();
        if !l.is_empty() {
            out.push(l.to_string());
        }
    }
    Ok(out)
}

/// Truncate `s` to at most `n` characters (the script's `${s:0:9}` /
/// `${impl:0:23}` is character-based).
fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// A temporary directory removed when dropped.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> std::io::Result<Self> {
        let base = std::env::temp_dir();
        let unique = format!("fb-crossx-{}-{}", std::process::id(), unique_counter());
        let path = base.join(unique);
        std::fs::create_dir_all(&path)?;
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

/// Process-local counter for unique temp dir names (no extra dependencies).
fn unique_counter() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static C: AtomicU64 = AtomicU64::new(0);
    C.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use farmerbob_core::crossx::Matrix;

    /// A 2×2 matrix used to pin the table format byte-for-byte.
    ///   (alpha, alpha) = pass,  (alpha, beta) = fail
    ///   (beta,  alpha) = nocomp, (beta,  beta) = pass
    fn small_matrix() -> (Vec<String>, Matrix) {
        let arms = vec!["alpha".to_string(), "beta".to_string()];
        let cells = vec![Cell::Pass, Cell::Fail, Cell::Error, Cell::Pass];
        let m = Matrix::new(arms.clone(), arms.clone(), cells).unwrap();
        (arms, m)
    }

    #[test]
    fn matrix_table_format_is_byte_for_byte() {
        let (arms, m) = small_matrix();
        let got = format_matrix(&arms, &m);
        // Built with the script's exact printf widths (%-24s header, %-10s cells,
        // ${s:0:9} / ${impl:0:23} truncation) so trailing spaces are pinned too.
        let want = format!("{:<24}", "impl \\ suite")
            + &format!("{:<10}", "alpha")
            + &format!("{:<10}", "beta")
            + "\n"
            + &format!("{:<24}", "alpha")
            + &format!("{:<10}", " .")
            + &format!("{:<10}", " FAIL")
            + "\n"
            + &format!("{:<24}", "beta")
            + &format!("{:<10}", " nocomp")
            + &format!("{:<10}", " .")
            + "\n";
        assert_eq!(
            got, want,
            "matrix table must match the script's printf exactly"
        );
    }

    #[test]
    fn arm_passes_gate_includes_all_when_absent() {
        let absent = Measurement::<BTreeSet<String>>::instrument_failed("no score");
        assert!(arm_passes_gate(&absent, "anyone"));
        let empty = Measurement::observed(BTreeSet::new());
        assert!(arm_passes_gate(&empty, "anyone"));
    }

    #[test]
    fn arm_passes_gate_excludes_unnamed_arms() {
        let mut set = BTreeSet::new();
        set.insert("kept".to_string());
        let observed = Measurement::observed(set);
        assert!(arm_passes_gate(&observed, "kept"));
        assert!(!arm_passes_gate(&observed, "dropped"));
    }

    #[test]
    fn diagonal_bad_reports_only_non_passing_own_suites() {
        let arms = vec!["a".to_string(), "b".to_string()];
        // a passes itself, b fails itself.
        let cells = vec![Cell::Pass, Cell::Fail, Cell::Pass, Cell::Fail];
        let m = Matrix::new(arms.clone(), arms.clone(), cells).unwrap();
        assert_eq!(diagonal_bad(&arms, &m), vec!["b(fail)".to_string()]);

        // All-pass diagonal: nothing reported.
        let ok = vec![Cell::Pass, Cell::Error, Cell::Error, Cell::Pass];
        let m = Matrix::new(arms.clone(), arms.clone(), ok).unwrap();
        assert!(diagonal_bad(&arms, &m).is_empty());
    }

    #[test]
    fn detect_partition_finds_two_self_consistent_camps() {
        let arms = vec![
            "A".to_string(),
            "B".to_string(),
            "C".to_string(),
            "D".to_string(),
        ];
        // Camp {A,B} passes within, fails across; {C,D} likewise.
        let cells = vec![
            Cell::Pass,
            Cell::Pass,
            Cell::Fail,
            Cell::Fail,
            Cell::Pass,
            Cell::Pass,
            Cell::Fail,
            Cell::Fail,
            Cell::Fail,
            Cell::Fail,
            Cell::Pass,
            Cell::Pass,
            Cell::Fail,
            Cell::Fail,
            Cell::Pass,
            Cell::Pass,
        ];
        let m = Matrix::new(arms.clone(), arms.clone(), cells).unwrap();
        assert_eq!(
            detect_partition(&arms, &m),
            Some("{A,B} | {C,D}".to_string())
        );
    }

    #[test]
    fn detect_partition_is_none_for_few_arms() {
        let arms = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let cells = vec![Cell::Pass; 9];
        let m = Matrix::new(arms.clone(), arms.clone(), cells).unwrap();
        assert_eq!(detect_partition(&arms, &m), None);
    }

    #[test]
    fn detect_partition_is_none_when_single_camp() {
        // All four arms pass every suite: one camp, so no partition.
        let arms = vec![
            "A".to_string(),
            "B".to_string(),
            "C".to_string(),
            "D".to_string(),
        ];
        let cells = vec![Cell::Pass; 16];
        let m = Matrix::new(arms.clone(), arms.clone(), cells).unwrap();
        assert_eq!(detect_partition(&arms, &m), None);
    }

    #[test]
    fn compute_stats_survival_is_absent_when_no_discriminating_suite() {
        // All pass: nothing discriminates, survival absent.
        let arms = vec!["a".to_string(), "b".to_string()];
        let cells = vec![Cell::Pass, Cell::Pass, Cell::Pass, Cell::Pass];
        let m = Matrix::new(arms.clone(), arms.clone(), cells).unwrap();
        let stats = compute_stats(&arms, &m);
        for r in &stats {
            assert!(
                r.survival.absent().is_some(),
                "no discrimination => survival absent"
            );
            assert!(!r.suite_discriminating);
            assert_eq!(r.discovery, 0);
        }
    }

    #[test]
    fn compute_stats_one_of_n_is_a_defect_not_overfit() {
        // a's suite fails only b; b's suite fails only a.
        let arms = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let cells = vec![
            Cell::Pass,
            Cell::Fail,
            Cell::Pass,
            Cell::Fail,
            Cell::Pass,
            Cell::Pass,
            Cell::Pass,
            Cell::Pass,
            Cell::Pass,
        ];
        let m = Matrix::new(arms.clone(), arms.clone(), cells).unwrap();
        let stats = compute_stats(&arms, &m);

        let a = stats.iter().find(|r| r.arm == "a").unwrap();
        assert!(a.suite_discriminating);
        assert!(!a.suite_overfitted);
        // a's own implementation fails b's discriminating suite, so survival is 0/1.
        assert_eq!(a.survival.value().copied(), Some(0.0));
        assert_eq!(a.discovery, 1);

        let b = stats.iter().find(|r| r.arm == "b").unwrap();
        assert!(b.suite_discriminating);
        assert_eq!(b.discovery, 1);
    }

    #[test]
    fn compute_stats_overfit_discovery_discounted() {
        // a's suite breaks every other impl it compiles against -> over-fitted.
        let arms = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let cells = vec![
            Cell::Pass,
            Cell::Pass,
            Cell::Pass,
            Cell::Fail,
            Cell::Pass,
            Cell::Pass,
            Cell::Fail,
            Cell::Pass,
            Cell::Pass,
        ];
        let m = Matrix::new(arms.clone(), arms.clone(), cells).unwrap();
        let stats = compute_stats(&arms, &m);
        let a = stats.iter().find(|r| r.arm == "a").unwrap();
        assert!(a.suite_overfitted);
        assert_eq!(a.discovery, 0, "over-fit suite contributes no discovery");
    }

    #[test]
    fn compute_stats_nocompile_excluded_from_denominators() {
        // a's suite compiles only against c (b is nocompile). a fails c.
        let arms = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let cells = vec![
            Cell::Pass,
            Cell::Error,
            Cell::Fail,
            Cell::Pass,
            Cell::Pass,
            Cell::Pass,
            Cell::Pass,
            Cell::Pass,
            Cell::Pass,
        ];
        let m = Matrix::new(arms.clone(), arms.clone(), cells).unwrap();
        let stats = compute_stats(&arms, &m);
        let a = stats.iter().find(|r| r.arm == "a").unwrap();
        assert!(!a.suite_discriminating); // compiles(a) = {c}, len 1
        // a's implementation fails c's discriminating suite, so survival is 0/1,
        // and b's nocompile against a is excluded from a's denominators.
        assert_eq!(a.survival.value().copied(), Some(0.0));
        assert_eq!(a.discovery, 0);
        assert_eq!(a.api_incompatible_with, 0);
        // b's suite could not compile against a (cell(a,b) = nocompile).
        let b = stats.iter().find(|r| r.arm == "b").unwrap();
        assert_eq!(b.api_incompatible_with, 1, "a is nocompile vs b's suite");
    }

    #[test]
    fn summary_table_format_is_byte_for_byte() {
        let (arms, m) = small_matrix();
        let stats = compute_stats(&arms, &m);
        let got = format_summary(&arms, &stats);
        // alpha: no discriminating foreign suite, its own suite is nocompile vs beta
        //        => survival absent, api_incompatible_with = 1.
        // beta: its suite breaks every impl it compiles against (only alpha) => over-fitted,
        //       discovery discounted to 0.
        let want = format!(
            "{:<24}{:>10}{:>11}{:>14}  SUITE\n",
            "ARM", "SURVIVAL", "DISCOVERY", "API-INCOMPAT"
        ) + &format!(
            "{:<24}{:>10}{:>11}{:>14}  {}\n",
            "alpha", "n/a", 0, 1, "no signal"
        ) + &format!(
            "{:<24}{:>10}{:>11}{:>14}  {}\n",
            "beta", "n/a", 0, 0, "OVER-FITTED"
        );
        assert_eq!(
            got, want,
            "summary table must match the script's python format"
        );
    }

    #[test]
    fn json_round_trips_with_indent_one() {
        let (arms, m) = small_matrix();
        let stats = compute_stats(&arms, &m);
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("out.json");
        write_json(&p, &stats).unwrap();
        let txt = std::fs::read_to_string(&p).unwrap();
        assert!(txt.starts_with("{\n \""), "top level indented by one space");
        assert!(
            txt.contains("\"survival\": null"),
            "absent survival serialises as null"
        );
        let _: serde_json::Value = serde_json::from_str(&txt).unwrap();
    }

    #[test]
    fn graft_text_pipeline_renames_mod_tests_and_injects_uses() {
        let src = "\
use std::path::PathBuf;
use std::time::Utc;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn it_works() { assert_eq!(1, 1); }
}
";
        let uses = top_level_uses(src);
        assert_eq!(uses.len(), 2);
        let body = test_body(src, 3);
        assert!(body.contains("mod xtests_3"));
        assert!(!body.contains("mod tests "));
        assert!(body.contains("use super::*"));
        let pruned = prune_uses(uses, &body);
        // No explicit `use` duplicates a body line, so both survive.
        assert_eq!(pruned.len(), 2);
        let assembled = assemble_suite(&pruned, &body);
        assert!(assembled.contains("mod xtests_3"));
        assert!(assembled.contains("use std::path::PathBuf"));
    }

    #[test]
    fn prune_uses_drops_already_declared_import() {
        let src = "use std::collections::HashMap;\n#[cfg(test)]\nmod tests {\n    use std::collections::HashMap;\n}\n";
        let uses = top_level_uses(src);
        assert_eq!(uses.len(), 1);
        let body = test_body(src, 0);
        let pruned = prune_uses(uses, &body);
        assert!(
            pruned.is_empty(),
            "duplicate explicit import must be dropped (E0252)"
        );
    }

    #[test]
    fn slugify_matches_tr_script() {
        assert_eq!(slugify("crates/foo/src/bar.rs"), "crates_foo_src_bar_rs");
        assert_eq!(slugify("a-b_c.d"), "a_b_c_d");
    }

    #[test]
    fn strip_test_modules_drops_everything_after_cfg_test() {
        let src = "line1\n#[cfg(test)]\nmod tests {\n    #[test]\n fn x(){}\n}\n";
        let kept: String = src
            .lines()
            .take_while(|l| !l.contains("#[cfg(test)]"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(kept, "line1");
    }

    #[test]
    fn parse_declared_parses_creates_and_modifies() {
        let md = "\
# Task
<!-- fb:creates crates/foo/src/a.rs -->
text
<!-- fb:modifies crates/foo/src/b.rs -->
<!-- fb:creates crates/foo/src/a.rs -->  duplicate
";
        let got = parse_declared(md);
        assert_eq!(
            got,
            vec![
                "crates/foo/src/a.rs".to_string(),
                "crates/foo/src/b.rs".to_string()
            ]
        );
    }
}
