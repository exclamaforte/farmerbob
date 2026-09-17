//! The objective tier: every machine-computed metric, per candidate, per task.
//!
//! Port of `fb-objective.sh`. No model opinion anywhere -- this is the reward
//! signal for autoresearch; the subjective tier runs only where this table fails
//! to discriminate.
//!
//! Two behaviours of the shell original are load-bearing and reproduced exactly:
//!
//! 1. The `crates_touched` field of a score record is emitted under the wire name
//!    `crates`. `objective.json` is what `fb pareto`, `fb select` and the bandit's
//!    posteriors all read, so the rename is not cosmetic -- a missing or mis-named
//!    field is a silent wire-format break.
//!
//! 2. The row set is whatever the score files contain, filtered by the optional
//!    task argument. The classifier below never adds or drops a row; it only
//!    rewrites a row's `outcome`.
//!
//! The spec's central requirement -- that "measured zero" and "not measured" be
//! unrepresentable as the same value -- is met by carrying the per-run numeric
//! metrics as [`Measurement`]. The script turned a missing metric into a zero in
//! exactly one place (the unanimity reclassification, `x.get("lines") or 0`); a
//! `Missing` line there is now correctly *not* zero, so an unmeasured line count
//! cannot falsely indict a task as invalid.

use std::collections::BTreeMap;
use std::path::Path;

use farmerbob_core::measurement::Measurement;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::paths;
use crate::sources::Registry;

/// Patterns that, appearing in the first lines of a run's own log, mean the
/// provider refused on quota grounds. Anchored to the launcher's own error prefix
/// so an agent *discussing* quotas in a task about quotas is not misclassified.
const LIMIT_PATTERNS: &[&str] = &[
    "error: individual quota reached",
    "error: quota exceeded",
    "error: rate limit exceeded",
    "error: 429",
    "usage limit reached",
    // OpenRouter, when the KEY's total spend cap is reached rather than a per-minute rate:
    //   Error: Key limit exceeded (total limit). Manage it using https://openrouter.ai/...
    // Absent from this list until 2026-09-17, when it took out all 13 OpenRouter arms at
    // once and the harness recorded each refusal as a NO-OP -- an arm that produced nothing.
    // Two false NO-OPs reached the posterior in four and nine seconds respectively, which is
    // less time than it takes an agent to read its prompt.
    //
    // This is farmerbob-h04 again, one provider and one error string later. The lesson that
    // list did not learn is that it is a list: every refusal NOT enumerated here is scored
    // as a failure of the model. The default is the dangerous one, and the set of ways a
    // provider can say no is open. (farmerbob-2ve -- superset status on enumerated lists.)
    "error: key limit exceeded",
];

/// Patterns that, appearing in the head of a run's log, mean the harness itself
/// (credential, endpoint, request shape) is at fault, not the model.
const INFRA_PATTERNS: &[&str] = &[
    "incorrect api key provided",
    "wrong_api_format",
    // Reproduced verbatim from the script, escaped-quote artifact and all: the
    // Python tuple element `"is unsupported\",",` parses to the string
    // `is unsupported",`, which is what is actually matched against the log.
    "is unsupported\",",
    "invalid_request_error",
    "401 unauthorized",
];

/// Run the objective tier.
///
/// `only`, when `Some` and non-empty, restricts the report to that single task,
/// mirroring the script's single positional argument (`${1:-}`). Returns the
/// process exit code: `0` on success, `1` on a setup error that prevents the
/// report from being produced.
pub fn run_cmd(only: Option<String>) -> i32 {
    let logs = paths::logs();
    let repo = paths::repo();

    let registry = match Registry::load(&repo.join("sources.toml")) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("fb objective: cannot load registry: {e}");
            return 1;
        }
    };

    let score = load_index(&logs, ".score.json");
    let crossx = load_index(&logs, ".crossx.json");
    let defects = load_index(&logs, ".defects.json");

    // Reads a log file as lossy, lowercased text. Lossy like the script's
    // `errors="replace"`; lowercased because every pattern is lowercased.
    let read_log = |p: &Path| -> Option<String> {
        std::fs::read(p)
            .ok()
            .map(|b| String::from_utf8_lossy(&b).to_lowercase())
    };

    let mut rows: Vec<Row> = Vec::new();
    for (task, entries) in &score {
        if let Some(o) = &only && !o.is_empty() && task != o {
            continue;
        }
        let Some(entries_arr) = entries.as_array() else {
            continue;
        };
        if entries_arr.is_empty() {
            continue;
        }
        let cxall = crossx.get(task).and_then(Value::as_object);
        let n = cxall.map_or(0, |m| m.len().saturating_sub(1));

        for e in entries_arr {
            let arm = e
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let rec = load_json(&logs.join(format!("{task}--{arm}.json"))).unwrap_or(Value::Null);
            let src = registry.get(&arm);

            let cx = cxall.and_then(|m| m.get(&arm));
            let df = defects
                .get(task)
                .and_then(Value::as_object)
                .and_then(|m| m.get("arms"))
                .and_then(Value::as_object)
                .and_then(|m| m.get(&arm));

            // Portability denominator comes from the crossx run's OWN arm count,
            // not the score file: different runs covered different candidate
            // sets, which produced negative "3/3 minus 3" values.
            let portability = match (cx, n) {
                (Some(cx), n) if n > 0 => {
                    let api = cx
                        .get("api_incompatible_with")
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    let molecule = (n as i64 - api).max(0);
                    Some(format!("{molecule}/{n}"))
                }
                _ => None,
            };

            let defect_sens = match df {
                Some(df) => {
                    let comparable = df.get("comparable").and_then(Value::as_i64).unwrap_or(0);
                    if comparable == 0 {
                        None
                    } else {
                        let caught = df.get("caught").map(Value::to_string).unwrap_or_default();
                        Some(format!("{caught}/{comparable}"))
                    }
                }
                None => None,
            };

            let price = match src {
                Some(s) if s.quota.as_deref() != Some("plan") => s.price_in,
                _ => None,
            };

            let outcome = classify_outcome(&rec, e.get("verdict"), &read_log);
            rows.push(Row {
                task: task.clone(),
                arm,
                verdict: e.get("verdict").cloned(),
                tests: measured(e.get("tests_run")),
                clippy: measured(e.get("clippy")),
                lines: measured(e.get("lines")),
                crates: e.get("crates_touched").cloned(),
                portability,
                defect_sens,
                secs: measured(e.get("duration_s")),
                mem_mb: measured(rec.get("mem_peak_mb")),
                price,
                outcome,
            });
        }
    }

    // UNANIMITY INDICTS THE TASK, NOT THE FIELD. When every arm on a task wrote
    // zero lines, the likely cause is an already-delivered or unreachable
    // deliverable, not four models each choosing to do nothing. A line count
    // that is `Missing` is NOT zero, so an unmeasured candidate does not trigger
    // this -- that is the bug the port exists to remove.
    let by_task = unanimity_groups(&rows);
    for idxs in by_task.values() {
        let arm_idxs: Vec<usize> = idxs
            .iter()
            .copied()
            .filter(|i| rows[*i].outcome == "arm_result")
            .collect();
        if arm_idxs.len() >= 2 && arm_idxs.iter().all(|i| lines_is_zero(&rows[*i])) {
            for i in arm_idxs {
                rows[i].outcome = "task_invalid".to_string();
            }
        }
    }

    let out_path = logs.join("objective.json");
    let array: Vec<Value> = rows.iter().map(row_to_json).collect();
    if let Err(e) = write_json(&out_path, &Value::Array(array)) {
        eprintln!("fb objective: cannot write {}: {e}", out_path.display());
        return 1;
    }

    let stdout = render_report(&rows, rows.len(), &out_path);
    println!("{stdout}");
    0
}

/// One candidate-run's metrics, as the objective tier records them.
struct Row {
    task: String,
    arm: String,
    /// The score file's `verdict` (a raw string such as "PASS" / "NO-COMPILE").
    verdict: Option<Value>,
    tests: Measurement<Value>,
    clippy: Measurement<Value>,
    lines: Measurement<Value>,
    /// Emitted under the wire name `crates` (renamed from `crates_touched`).
    crates: Option<Value>,
    portability: Option<String>,
    defect_sens: Option<String>,
    secs: Measurement<Value>,
    mem_mb: Measurement<Value>,
    /// `None` means the arm is free (plan arm, or no price recorded).
    price: Option<f64>,
    outcome: String,
}

/// A missing field becomes `Missing` with a stated reason; a present value is
/// `Observed`. `null` in the source is treated as absent, never as a zero.
fn measured(v: Option<&Value>) -> Measurement<Value> {
    match v {
        Some(Value::Null) | None => {
            Measurement::nothing_to_measure("field absent from the score or run record")
        }
        Some(other) => Measurement::observed(other.clone()),
    }
}

/// Whether a row's line count is a measured zero. `Missing` is deliberately NOT
/// zero: an unmeasured candidate must not be read as having written nothing.
fn lines_is_zero(r: &Row) -> bool {
    match r.lines.value() {
        Some(Value::Number(n)) => n.as_f64() == Some(0.0),
        _ => false,
    }
}

/// Group row indices by task, for the unanimity pass.
fn unanimity_groups(rows: &[Row]) -> BTreeMap<String, Vec<usize>> {
    let mut by_task: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, r) in rows.iter().enumerate() {
        by_task.entry(r.task.clone()).or_default().push(i);
    }
    by_task
}

/// Decide a row's outcome, reproducing the script's `if/elif` chain in order.
///
/// `rec` is the run record (possibly `Value::Null` when the file is absent);
/// `score_verdict` is the score file's `verdict` (used only for the
/// `TASK-INVALID` check). `read_log` returns a run's log text, or `None`.
fn classify_outcome(
    rec: &Value,
    score_verdict: Option<&Value>,
    read_log: &dyn Fn(&Path) -> Option<String>,
) -> String {
    // An explicit, non-empty outcome_class is authoritative.
    if let Some(Value::String(s)) = rec.get("outcome_class") && !s.is_empty() {
        return s.clone();
    }
    if score_verdict_is(score_verdict, "TASK-INVALID") {
        return "task_invalid".to_string();
    }
    if let Some(rc) = rec.get("rc").and_then(Value::as_i64) && (rc == 143 || rc == 137) {
        return "orchestrator_cancelled".to_string();
    }
    // 127 is "command not found". It is always the harness -- a launcher binary that is not
    // installed, or an arm with no branch in fb-dispatch's launcher table -- and never the
    // model, which was not reached. claude-sonnet was re-enabled on 2026-09-17, dispatched,
    // and came back rc=127 "unknown source" in 0 seconds because fb-dispatch has its own
    // hardcoded launcher table that does not include it. That was recorded as the ARM
    // producing nothing.
    if let Some(127) = rec.get("rc").and_then(Value::as_i64) {
        return "infrastructure".to_string();
    }
    if quota_blocked(rec, read_log) {
        return "quota_limited".to_string();
    }
    if permission_killed(rec, read_log) || harness_failed(rec, read_log) {
        return "infrastructure".to_string();
    }
    if matches!(rec.get("verdict"), None | Some(Value::Null)) {
        return "unknown".to_string();
    }
    "arm_result".to_string()
}

fn score_verdict_is(v: Option<&Value>, s: &str) -> bool {
    matches!(v, Some(Value::String(x)) if x == s)
}

/// True when the run's own log opens with a provider refusal.
fn quota_blocked(rec: &Value, read_log: &dyn Fn(&Path) -> Option<String>) -> bool {
    let Some(text) = log_head(rec, read_log) else {
        return false;
    };
    // Lowercased as well as ANSI-stripped. Every pattern in the list is lowercase and real
    // launchers capitalise: "Error: Key limit exceeded". Nothing lowercased the haystack, so
    // a capitalised refusal never matched any pattern here. The unit tests all passed because
    // they were written with lowercase fixtures, which is how a check can be wrong for months
    // while its tests are green -- the fixture agreed with the code instead of with the logs.
    head_starts_with_any(&text, 5, LIMIT_PATTERNS)
}

/// True when the harness (credential, endpoint, request shape) killed the run.
fn harness_failed(rec: &Value, read_log: &dyn Fn(&Path) -> Option<String>) -> bool {
    let Some(text) = log_head(rec, read_log) else {
        return false;
    };
    // Lowercased for the same reason as quota_blocked: these patterns are lowercase and
    // launchers capitalise. "Incorrect API key provided" never matched
    // "incorrect api key provided".
    head_starts_with_any(&text, 12, INFRA_PATTERNS)
}

/// True when the launcher refused a tool call touching an external directory.
fn permission_killed(rec: &Value, read_log: &dyn Fn(&Path) -> Option<String>) -> bool {
    let Some(text) = log_head(rec, read_log) else {
        return false;
    };
    let body: String = text.chars().take(200_000).collect();
    body.contains("permission requested: external_directory")
        && body.contains("rejected permission to use this specific tool call")
}

/// The lowercased log text of `rec`'s `log` file, or `None` when there is no
/// usable log.
fn log_head(rec: &Value, read_log: &dyn Fn(&Path) -> Option<String>) -> Option<String> {
    let path = rec.get("log").and_then(Value::as_str)?;
    read_log(Path::new(path))
}

/// True when any of the first `n` lines BEGINS with one of `patterns`, after ANSI stripping,
/// trimming and lowercasing.
///
/// `starts_with` on a line, not `contains` on the whole head. The difference is the entire
/// guard, and it was learned twice:
///
/// The patterns are anchored on a launcher's `error: ` prefix so that an agent DISCUSSING
/// quotas in a task about quotas is not read as one being refused. That anchoring turned out
/// to be worth nothing on its own. On 2026-09-17 an arm completed the task of FIXING refusal
/// detection, and opened its summary with
///
///     Done. Provider-refusal detection ... the anchored pattern that missed the
///     2026-09-17 OpenRouter incident (`error: rate limit exceeded`) ...
///
/// which contains the anchor, quoted. A passing run with 396 new lines was recorded as
/// `quota_limited` and dropped out of the arm results entirely.
///
/// It could not fire before, only because of a second bug: nothing lowercased the haystack, so
/// no capitalised launcher error matched either. Fixing the casing switched on the true
/// positives and this false one together.
///
/// A real refusal is the launcher's own first utterance and BEGINS its line. A quotation sits
/// inside a sentence. That is the distinction with actual force, and it is positional.
fn head_starts_with_any(text: &str, n: usize, patterns: &[&str]) -> bool {
    text.lines().take(n).any(|line| {
        let flat = strip_ansi(line).trim().to_lowercase();
        patterns.iter().any(|p| flat.starts_with(p))
    })
}

fn first_lines(text: &str, n: usize) -> String {
    text.lines()
        .take(n)
        .map(strip_ansi)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Remove ANSI escape sequences from one line.
///
/// Every pattern in LIMIT_PATTERNS and INFRA_PATTERNS is anchored to a launcher's `error: `
/// prefix, on the sound reasoning that an agent DISCUSSING quotas in a task about quotas must
/// not be misread as one being refused. That anchoring is silently defeated by colour. The
/// OpenRouter launcher emits
///
///     \x1b[91m\x1b[1mError: \x1b[0mKey limit exceeded (total limit).
///
/// with the reset sequence sitting BETWEEN the prefix and the message, so the literal bytes
/// are `Error: \x1b[0mKey limit exceeded` and no `error: <message>` pattern can ever match.
/// Adding the pattern was not enough and adding more patterns would not have helped; every
/// existing entry is defeatable the same way the moment a launcher colourises its output.
///
/// Found 2026-09-17, when an OpenRouter key spend cap refused all 13 of its arms and each
/// refusal was recorded as the model producing nothing.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        // CSI: ESC [ <params> <final byte in @-~>. Anything else after ESC: drop the ESC and
        // the single byte that follows, which covers the short two-character sequences.
        if let Some('[') = chars.next() {
            for c in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        }
    }
    out
}

/// Render the full stdout report, byte-for-byte with the shell original.
fn render_report(rows: &[Row], count: usize, out_path: &Path) -> String {
    let mut s = String::new();
    s.push_str(&header_line());
    s.push('\n');
    s.push_str(&"-".repeat(header_line().len()));
    for r in rows {
        s.push('\n');
        s.push_str(&row_line(r));
    }
    s.push('\n');
    s.push_str(&format!(
        "\n{count} candidate-runs -> {}",
        out_path.display()
    ));
    s.push_str("\n\nwhere the objective tier does NOT separate candidates (subjective tier needed):");
    for (task, n) in non_discriminating(rows) {
        s.push('\n');
        s.push_str(&format!("  {task:<14}{n} candidates pass the gate"));
    }
    s
}

/// Tasks where more than one arm both reached `arm_result` and passed the gate.
fn non_discriminating(rows: &[Row]) -> Vec<(String, usize)> {
    let mut by_task: BTreeMap<String, usize> = BTreeMap::new();
    for r in rows {
        if r.outcome == "arm_result" && score_verdict_is(r.verdict.as_ref(), "PASS") {
            *by_task.entry(r.task.clone()).or_insert(0) += 1;
        }
    }
    by_task
        .into_iter()
        .filter(|(_, n)| *n > 1)
        .collect()
}

fn header_line() -> String {
    format!(
        "{:<14}{:<22}{:<11}{:>6}{:>7}{:>7}{:>7}{:>8}{:>6}{:>6}{:>7}  OUTCOME",
        "TASK", "ARM", "VERDICT", "TESTS", "CLIPPY", "LINES", "PORT", "DEFECT", "SECS", "MEM", "$/1M"
    )
}

fn row_line(r: &Row) -> String {
    let verdict = match &r.verdict {
        None => "None".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
    };
    let price = match r.price {
        None => "free".to_string(),
        Some(p) => format!("{p:.2}"),
    };
    format!(
        "{:<14}{:<22}{:<11}{:>6}{:>7}{:>7}{:>7}{:>8}{:>6}{:>6}{:>7}  {}",
        r.task,
        r.arm,
        verdict,
        disp_present(&r.tests),
        disp_present(&r.clippy),
        disp_present(&r.lines),
        disp_dashable(r.portability.as_ref()),
        disp_dashable(r.defect_sens.as_ref()),
        disp_present(&r.secs),
        disp_dash_measure(&r.mem_mb),
        price,
        r.outcome
    )
}

/// A JSON value rendered for the table: strings unquoted (mirroring Python
/// `str`), everything else via its JSON form. `Missing` renders as `-`.
fn disp_present(m: &Measurement<Value>) -> String {
    match m.value() {
        Some(v) => disp_value(v),
        None => "-".to_string(),
    }
}

fn disp_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `None` or empty string renders as `-`; everything else verbatim. Mirrors the
/// script's `str(x or '-')`.
fn disp_dashable(s: Option<&String>) -> String {
    s.filter(|x| !x.is_empty())
        .map_or_else(|| "-".to_string(), |x| x.clone())
}

/// Like [`disp_present`] but a measured zero (or empty string) also renders as
/// `-`, mirroring the script's `str(r['mem_mb'] or '-')`.
fn disp_dash_measure(m: &Measurement<Value>) -> String {
    match m.value() {
        None => "-".to_string(),
        Some(Value::Number(n)) if n.as_f64() == Some(0.0) => "-".to_string(),
        Some(Value::String(s)) if s.is_empty() => "-".to_string(),
        Some(v) => disp_value(v),
    }
}

/// Serialize one row to the exact wire object the script emits: same keys, same
/// order, `Missing`/absent as JSON `null`.
fn row_to_json(r: &Row) -> Value {
    let mut m = Map::new();
    m.insert("task".into(), Value::String(r.task.clone()));
    m.insert("arm".into(), Value::String(r.arm.clone()));
    m.insert("verdict".into(), r.verdict.clone().unwrap_or(Value::Null));
    m.insert("tests".into(), measure_to_value(&r.tests));
    m.insert("clippy".into(), measure_to_value(&r.clippy));
    m.insert("lines".into(), measure_to_value(&r.lines));
    m.insert("crates".into(), r.crates.clone().unwrap_or(Value::Null));
    m.insert("portability".into(), opt_str(&r.portability));
    m.insert("defect_sens".into(), opt_str(&r.defect_sens));
    m.insert("secs".into(), measure_to_value(&r.secs));
    m.insert("mem_mb".into(), measure_to_value(&r.mem_mb));
    m.insert(
        "price".into(),
        match r.price {
            Some(p) => Value::from(p),
            None => Value::Null,
        },
    );
    m.insert("outcome".into(), Value::String(r.outcome.clone()));
    Value::Object(m)
}

fn measure_to_value(m: &Measurement<Value>) -> Value {
    m.value().cloned().unwrap_or(Value::Null)
}

fn opt_str(o: &Option<String>) -> Value {
    match o {
        Some(s) => Value::String(s.clone()),
        None => Value::Null,
    }
}

/// Write `value` as JSON with indent 1, matching `json.dump(..., indent=1)`.
fn write_json(path: &Path, value: &Value) -> Result<(), std::io::Error> {
    let mut buf = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(
        &mut buf,
        serde_json::ser::PrettyFormatter::with_indent(b" "),
    );
    value
        .serialize(&mut ser)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, buf)
}

/// Load every `<prefix>.json` in `dir` keyed by the task name (the basename with
/// the suffix stripped). A file that cannot be read or parsed is skipped, exactly
/// as the script's `jload` returns `None` for it.
fn load_index(dir: &Path, suffix: &str) -> BTreeMap<String, Value> {
    let mut map = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return map;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(task) = name.strip_suffix(suffix) && let Some(value) = load_json(&entry.path()) {
            map.insert(task.to_string(), value);
        }
    }
    map
}

/// Parse a JSON file, or `None` on any error (file missing/unreadable/unparseable).
fn load_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Build a row with every field supplied, for format pinning.
    fn row(
        task: &str,
        arm: &str,
        verdict: Option<&str>,
        tests: Measurement<Value>,
        clippy: Measurement<Value>,
        lines: Measurement<Value>,
        crates: Option<Value>,
        portability: Option<&str>,
        defect_sens: Option<&str>,
        secs: Measurement<Value>,
        mem_mb: Measurement<Value>,
        price: Option<f64>,
        outcome: &str,
    ) -> Row {
        Row {
            task: task.to_string(),
            arm: arm.to_string(),
            verdict: verdict.map(|v| Value::String(v.to_string())),
            tests,
            clippy,
            lines,
            crates,
            portability: portability.map(str::to_string),
            defect_sens: defect_sens.map(str::to_string),
            secs,
            mem_mb,
            price,
            outcome: outcome.to_string(),
        }
    }

    #[test]
    fn header_is_fixed_width_and_words() {
        let h = header_line();
        assert_eq!(
            h.len(),
            14 + 22 + 11 + 6 + 7 + 7 + 7 + 8 + 6 + 6 + 7 + 2 + "OUTCOME".len()
        );
        assert!(h.starts_with("TASK"), "got: {h:?}");
        assert!(h.contains("VERDICT"), "got: {h:?}");
        assert!(h.ends_with("OUTCOME"), "got: {h:?}");
        assert!(h.contains("$/1M"), "got: {h:?}");
    }

    #[test]
    fn row_format_is_byte_for_byte() {
        // A fully-populated row, exercised against the script's own column rules.
        let r = row(
            "gpu-lease",
            "glm-53-flash",
            Some("PASS"),
            Measurement::observed(json!(10)),
            Measurement::observed(json!("0")),
            Measurement::observed(json!(0)),
            Some(json!(2)),
            Some("3/4"),
            None,
            Measurement::observed(json!(120.5)),
            Measurement::observed(json!(512)),
            Some(1.25),
            "arm_result",
        );
        let line = row_line(&r);
        let expected = "gpu-lease     glm-53-flash          PASS           10      0      0    3/4       - 120.5   512   1.25  arm_result";
        assert_eq!(line, expected, "table format must match the script exactly");
    }

    #[test]
    fn missing_metrics_render_as_dash_not_zero() {
        let r = row(
            "t",
            "a",
            None,
            Measurement::not_attempted(),
            Measurement::nothing_to_measure("absent"),
            Measurement::observed(json!(7)),
            None,
            None,
            None,
            Measurement::observed(json!(3)),
            Measurement::not_attempted(),
            None,
            "arm_result",
        );
        let line = row_line(&r);
        // verdict None -> "None"; tests missing -> "-"; price None -> "free".
        assert!(line.contains("None"), "got: {line:?}");
        assert!(line.contains("  -  "), "missing metric must show dash: {line:?}");
        assert!(line.contains("free"), "missing price shows free: {line:?}");
        assert!(line.contains("7"), "observed lines shown: {line:?}");
    }

    #[test]
    fn zero_mem_renders_as_dash_like_the_script() {
        let r = row(
            "t", "a", Some("PASS"),
            Measurement::observed(json!(1)),
            Measurement::observed(json!("0")),
            Measurement::observed(json!(1)),
            Some(json!(1)),
            Some("1/1"), None,
            Measurement::observed(json!(1)),
            Measurement::observed(json!(0)),
            Some(0.0),
            "arm_result",
        );
        let line = row_line(&r);
        assert!(line.contains("  -  "), "measured 0 mem shows dash: {line:?}");
        assert!(line.contains("0.00"), "price 0.0 formatted: {line:?}");
    }

    #[test]
    fn crates_field_is_renamed_from_crates_touched() {
        let r = row(
            "t", "a", Some("PASS"),
            Measurement::observed(json!(1)),
            Measurement::observed(json!("0")),
            Measurement::observed(json!(1)),
            Some(json!(3)),
            None, None,
            Measurement::observed(json!(1)),
            Measurement::observed(json!(1)),
            Some(2.0),
            "arm_result",
        );
        let obj = row_to_json(&r);
        assert!(obj.get("crates").is_some(), "wire field is 'crates'");
        assert_eq!(obj.get("crates"), Some(&json!(3)));
        assert!(obj.get("crates_touched").is_none(), "original name must not leak");
        assert_eq!(obj.get("task"), Some(&json!("t")));
        assert_eq!(obj.get("arm"), Some(&json!("a")));
        assert_eq!(obj.get("outcome"), Some(&json!("arm_result")));
    }

    #[test]
    fn json_omits_nothing_and_uses_null_for_absent() {
        let r = row(
            "t", "a", None,
            Measurement::not_attempted(),
            Measurement::nothing_to_measure("x"),
            Measurement::observed(json!(1)),
            None,
            None, None,
            Measurement::observed(json!(1)),
            Measurement::not_attempted(),
            None,
            "arm_result",
        );
        let obj = row_to_json(&r);
        // serde_json::Map orders keys alphabetically without the preserve_order
        // feature, so we assert the field NAMES (the wire contract) rather than
        // their order; every consumer of objective.json reads it by key.
        for name in [
            "task", "arm", "verdict", "tests", "clippy", "lines", "crates",
            "portability", "defect_sens", "secs", "mem_mb", "price", "outcome",
        ] {
            assert!(obj.get(name).is_some(), "wire field {name} must be present");
        }
        assert!(obj.get("crates_touched").is_none(), "original name must not leak");
        assert_eq!(obj.get("tests"), Some(&Value::Null));
        assert_eq!(obj.get("crates"), Some(&Value::Null));
        assert_eq!(obj.get("price"), Some(&Value::Null));
    }

    #[test]
    fn outcome_respects_explicit_outcome_class() {
        let rec = json!({"outcome_class": "arm_result"});
        let out = classify_outcome(&rec, Some(&json!("PASS")), &_unused_log);
        assert_eq!(out, "arm_result");
        // An empty outcome_class falls through (it is falsy, like the script).
        let rec = json!({"outcome_class": ""});
        let out = classify_outcome(&rec, Some(&json!("TASK-INVALID")), &_unused_log);
        assert_eq!(out, "task_invalid");
    }

    #[test]
    fn outcome_task_invalid_from_score_verdict() {
        let rec = json!({});
        let out = classify_outcome(&rec, Some(&json!("TASK-INVALID")), &_unused_log);
        assert_eq!(out, "task_invalid");
    }

    #[test]
    fn outcome_orchestrator_cancelled_on_rc() {
        for rc in [143, 137] {
            let rec = json!({"rc": rc, "verdict": "PASS"});
            let out = classify_outcome(&rec, Some(&json!("PASS")), &_unused_log);
            assert_eq!(out, "orchestrator_cancelled", "rc={rc}");
        }
        let rec = json!({"rc": 1, "verdict": "PASS"});
        let out = classify_outcome(&rec, Some(&json!("PASS")), &_unused_log);
        assert_eq!(out, "arm_result", "ordinary rc is not a cancellation");
    }

    #[test]
    fn outcome_quota_limited_from_log_head() {
        let log = "header line one\nerror: individual quota reached for this account\nmore";
        let rec = json!({"log": "x.log", "verdict": "PASS"});
        let reader = |_: &Path| Some(log.to_string());
        let out = classify_outcome(&rec, Some(&json!("PASS")), &reader);
        assert_eq!(out, "quota_limited");
    }

    /// rc=127 is the harness failing to launch, never the model failing to work.
    #[test]
    fn command_not_found_is_infrastructure_not_an_arm_producing_nothing() {
        let rec = json!({"rc": 127, "verdict": "NO-OP", "log": "x.log"});
        let reader = |_: &Path| Some("unknown source claude-sonnet\n".to_string());
        assert_eq!(
            classify_outcome(&rec, Some(&json!("NO-OP")), &reader),
            "infrastructure"
        );
    }

    /// The false positive that line-anchoring exists to stop. Verbatim first line of
    /// refusal-norm--glm-53-flash.log, 2026-09-17: an arm REPORTING that it fixed refusal
    /// detection, quoting the pattern it fixed. A passing run with 396 new lines was recorded
    /// as quota_limited and dropped out of the arm results.
    #[test]
    fn an_arm_quoting_a_refusal_pattern_is_not_a_refusal() {
        let log = "Done. Provider-refusal detection in `limit_signal.rs` now survives colour: \
                   the anchored pattern that missed the 2026-09-17 OpenRouter incident \
                   (`error: rate limit exceeded`) now matches.\nmore output\n";
        let rec = json!({"log": "x.log", "verdict": "PASS", "rc": 0});
        let reader = |_: &Path| Some(log.to_string());
        assert_eq!(
            classify_outcome(&rec, Some(&json!("PASS")), &reader),
            "arm_result",
            "a pattern quoted mid-sentence is discussion, not a provider refusing"
        );
    }

    /// Both directions in one test, so neither can be fixed by breaking the other.
    #[test]
    fn a_real_refusal_still_fires_when_it_begins_its_line() {
        let refusal = "\u{1b}[91m\u{1b}[1mError: \u{1b}[0mRate limit exceeded: free-models-per-day.\n";
        let rec = json!({"log": "x.log", "verdict": "NO-OP", "rc": 1});
        let reader = |_: &Path| Some(refusal.to_string());
        assert_eq!(
            classify_outcome(&rec, Some(&json!("NO-OP")), &reader),
            "quota_limited"
        );
    }

    /// Verbatim from the head of port-status--or-hy3.log, 2026-09-17. The marker sits on the
    /// FIFTH line, exactly at the edge of the window quota_blocked reads -- the two escape
    /// sequences the launcher prints count as lines. One more banner line from any future
    /// launcher version and this refusal slides out of the window and is scored as an arm
    /// failure again, silently. Recorded here so the next person knows the margin is one line.
    #[test]
    fn an_openrouter_key_limit_is_a_refusal_not_a_no_op() {
        let log = "Using the OpenRouter credential from the global credential /home/x/.ori/credentials.json.\n\
                   \u{1b}[0m\n\
                   > build \u{b7} tencent/hy3\n\
                   \u{1b}[0m\n\
                   \u{1b}[91m\u{1b}[1mError: \u{1b}[0mKey limit exceeded (total limit). Manage it using https://openrouter.ai/\n";
        let rec = json!({"log": "x.log", "verdict": "NO-OP", "rc": 1});
        let reader = |_: &Path| Some(log.to_string());
        let out = classify_outcome(&rec, Some(&json!("NO-OP")), &reader);
        assert_eq!(
            out, "quota_limited",
            "a provider refusing on spend must never be recorded as an arm producing nothing"
        );
    }

    fn outcome_infrastructure_from_permission_kill() {
        let log = "permission requested: external_directory\n... rejected permission to use this specific tool call ...";
        let rec = json!({"log": "x.log", "verdict": "PASS"});
        let reader = |_: &Path| Some(log.to_string());
        let out = classify_outcome(&rec, Some(&json!("PASS")), &reader);
        assert_eq!(out, "infrastructure");
    }

    #[test]
    fn outcome_unknown_when_run_verdict_absent() {
        let rec = json!({"verdict": null});
        let out = classify_outcome(&rec, Some(&json!("PASS")), &_unused_log);
        assert_eq!(out, "unknown");
    }

    #[test]
    fn outcome_arm_result_when_evidenced() {
        let rec = json!({"verdict": "PASS", "rc": 0});
        let out = classify_outcome(&rec, Some(&json!("PASS")), &_unused_log);
        assert_eq!(out, "arm_result");
    }

    #[test]
    fn unanimity_reclassifies_only_when_all_wrote_zero() {
        // All three arm_result rows wrote zero lines -> the task is indicted.
        let mut rows = vec![
            row("gpu-lease", "a", Some("PASS"),
                Measurement::observed(json!(1)), Measurement::observed(json!("0")),
                Measurement::observed(json!(0)), Some(json!(1)), None, None,
                Measurement::observed(json!(1)), Measurement::observed(json!(1)), Some(1.0), "arm_result"),
            row("gpu-lease", "b", Some("PASS"),
                Measurement::observed(json!(1)), Measurement::observed(json!("0")),
                Measurement::observed(json!(0)), Some(json!(1)), None, None,
                Measurement::observed(json!(1)), Measurement::observed(json!(1)), Some(1.0), "arm_result"),
            row("gpu-lease", "c", Some("PASS"),
                Measurement::observed(json!(1)), Measurement::observed(json!("0")),
                Measurement::observed(json!(0)), Some(json!(1)), None, None,
                Measurement::observed(json!(1)), Measurement::observed(json!(1)), Some(1.0), "arm_result"),
        ];
        let by_task = unanimity_groups(&rows);
        apply_unanimity(&mut rows, &by_task);
        assert_eq!(rows[0].outcome, "task_invalid");
        assert_eq!(rows[1].outcome, "task_invalid");
        assert_eq!(rows[2].outcome, "task_invalid");

        // One arm wrote five lines: unanimity must NOT fire.
        let mut rows = vec![
            row("gpu-lease", "a", Some("PASS"),
                Measurement::observed(json!(1)), Measurement::observed(json!("0")),
                Measurement::observed(json!(0)), Some(json!(1)), None, None,
                Measurement::observed(json!(1)), Measurement::observed(json!(1)), Some(1.0), "arm_result"),
            row("gpu-lease", "b", Some("PASS"),
                Measurement::observed(json!(1)), Measurement::observed(json!("0")),
                Measurement::observed(json!(5)), Some(json!(1)), None, None,
                Measurement::observed(json!(1)), Measurement::observed(json!(1)), Some(1.0), "arm_result"),
        ];
        let by_task = unanimity_groups(&rows);
        apply_unanimity(&mut rows, &by_task);
        assert_eq!(rows[0].outcome, "arm_result", "a writer of 5 lines breaks unanimity");
        assert_eq!(rows[1].outcome, "arm_result");
    }

    #[test]
    fn unanimity_does_not_fire_on_single_arm() {
        let mut rows = vec![row("t", "a", Some("PASS"),
            Measurement::observed(json!(1)), Measurement::observed(json!("0")),
            Measurement::observed(json!(0)), Some(json!(1)), None, None,
            Measurement::observed(json!(1)), Measurement::observed(json!(1)), Some(1.0), "arm_result")];
        let by_task = unanimity_groups(&rows);
        apply_unanimity(&mut rows, &by_task);
        assert_eq!(rows[0].outcome, "arm_result", "one arm cannot be unanimous");
    }

    #[test]
    fn unanimity_treats_missing_lines_as_not_zero() {
        // The bug this port exists to remove: an unmeasured line count must not
        // be read as zero and falsely indict the task.
        let mut rows = vec![
            row("t", "a", Some("PASS"),
                Measurement::observed(json!(1)), Measurement::observed(json!("0")),
                Measurement::not_attempted(), Some(json!(1)), None, None,
                Measurement::observed(json!(1)), Measurement::observed(json!(1)), Some(1.0), "arm_result"),
            row("t", "b", Some("PASS"),
                Measurement::observed(json!(1)), Measurement::observed(json!("0")),
                Measurement::not_attempted(), Some(json!(1)), None, None,
                Measurement::observed(json!(1)), Measurement::observed(json!(1)), Some(1.0), "arm_result"),
        ];
        let by_task = unanimity_groups(&rows);
        apply_unanimity(&mut rows, &by_task);
        assert_eq!(rows[0].outcome, "arm_result", "unmeasured lines are not zero");
        assert_eq!(rows[1].outcome, "arm_result");
    }

    #[test]
    fn non_discriminating_lists_tasks_with_several_passers() {
        let rows = vec![
            row("t1", "a", Some("PASS"),
                Measurement::observed(json!(1)), Measurement::observed(json!("0")),
                Measurement::observed(json!(1)), Some(json!(1)), None, None,
                Measurement::observed(json!(1)), Measurement::observed(json!(1)), Some(1.0), "arm_result"),
            row("t1", "b", Some("PASS"),
                Measurement::observed(json!(1)), Measurement::observed(json!("0")),
                Measurement::observed(json!(1)), Some(json!(1)), None, None,
                Measurement::observed(json!(1)), Measurement::observed(json!(1)), Some(1.0), "arm_result"),
            row("t2", "a", Some("PASS"),
                Measurement::observed(json!(1)), Measurement::observed(json!("0")),
                Measurement::observed(json!(1)), Some(json!(1)), None, None,
                Measurement::observed(json!(1)), Measurement::observed(json!(1)), Some(1.0), "arm_result"),
        ];
        let list = non_discriminating(&rows);
        assert_eq!(list, vec![("t1".to_string(), 2)]);
    }

    // A log reader that is never consulted (outcome decided without a log).
    fn _unused_log(_: &Path) -> Option<String> {
        None
    }

    // Apply the unanimity pass against an externally built grouping, so the rule
    // can be tested without the filesystem.
    fn apply_unanimity(rows: &mut [Row], by_task: &BTreeMap<String, Vec<usize>>) {
        for idxs in by_task.values() {
            let arm_idxs: Vec<usize> = idxs
                .iter()
                .copied()
                .filter(|i| rows[*i].outcome == "arm_result")
                .collect();
            if arm_idxs.len() >= 2 && arm_idxs.iter().all(|i| lines_is_zero(&rows[*i])) {
                for i in arm_idxs {
                    rows[i].outcome = "task_invalid".to_string();
                }
            }
        }
    }
}




