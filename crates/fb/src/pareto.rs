//! `fb pareto` — cost against ability to complete real implementation tasks.
//!
//! Replaces the aggregation half of `fb-pareto.sh`, and exists mainly to give
//! `farmerbob_core::cost` and `farmerbob_core::pricing` the caller they never had.
//!
//! That absence had a price. `cost::RunCost::usd` has been `Option<f64>` since the day it was
//! merged, documented "`Some(0.0)` for a plan-based arm is a real zero; `None` is unmeasured"
//! — the exact distinction the shell could not express and got wrong. The shell aggregated
//! with `cost.get(run, 0.0)`, so three arms whose launchers write no cost store reported
//! $0.0000, and one of them was crowned on the Pareto frontier for being unmeasurable.
//! (beads farmerbob-6hc5, farmerbob-jd2.5)
//!
//! Token recovery (2026-09-17): arms whose launchers write no cost store now get a TOKENS
//! figure from the count each launcher prints in its own run log
//! (`farmerbob_core::cost::tokens_from_log`), with the launcher classified from the
//! registry's `cmd`. Recovery stops at tokens: it supplies no dollar figure, changes no
//! UNPRICED count, and moves no frontier.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use farmerbob_core::cost::{
    self as cost, ArmCost, Launcher, RunCost, aggregate, frontier, tokens_from_log, totals,
};
use farmerbob_core::measurement::Measurement;
use farmerbob_core::pricing::{Billing, canonical_model};

/// One run's measured spend, read from that run's own isolated opencode store.
///
/// Returns `None` when there is no store — which is a different fact from a store that says
/// zero, and is the whole reason this function does not return `f64`.
fn measured(base: &Path, run: &str) -> Option<(f64, u64)> {
    let db = base.join(format!("state/{run}/data/opencode/opencode.db"));
    if !db.exists() {
        return None;
    }
    let conn = rusqlite::Connection::open_with_flags(
        &db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .ok()?;
    let mut stmt = conn
        .prepare("select cost, tokens_input, tokens_output from session")
        .ok()?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, Option<f64>>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, Option<i64>>(2)?,
            ))
        })
        .ok()?;

    let mut usd = 0.0;
    let mut toks: u64 = 0;
    let mut any = false;
    for row in rows.flatten() {
        any = true;
        // A NULL cost inside a store that exists is still unmeasured. Measured across 135
        // stores there are currently zero of these, but folding it to 0.0 is how the dict
        // default became a $0.0000 on the frontier, and one latent copy is enough.
        usd += row.0?;
        toks += u64::try_from(row.1.unwrap_or(0)).unwrap_or(0)
            + u64::try_from(row.2.unwrap_or(0)).unwrap_or(0);
    }
    any.then_some((usd, toks))
}

/// Rows of `logs/objective.json`, the per-run record the scoring pipeline writes.
fn load_runs(base: &Path) -> Result<Vec<RunCost>, String> {
    let path = base.join("logs/objective.json");
    let body =
        fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let rows: Vec<serde_json::Value> = serde_json::from_str(&body)
        .map_err(|e| format!("{} is not valid JSON: {e}", path.display()))?;

    Ok(rows
        .iter()
        .filter_map(|r| {
            let arm = r.get("arm")?.as_str()?.to_string();
            let task = r.get("task")?.as_str()?.to_string();
            let verdict = r.get("verdict").and_then(|v| v.as_str()).unwrap_or("");
            let outcome = r.get("outcome").and_then(|v| v.as_str()).unwrap_or("");
            let m = measured(base, &format!("{task}--{arm}"));
            Some(RunCost {
                completed: verdict == "PASS",
                // Only an arm_result says anything about the arm. Infrastructure faults,
                // quota refusals and orchestrator kills are excluded from the posterior.
                counts_for_arm: outcome == "arm_result",
                // `billing` is what stops a metered arm with no price reading as free. The
                // registry is the only thing that knows, and this reader does not have it
                // here, so Unknown -- which is refused by `fully_measured` rather than being
                // silently treated as a real zero.  (bead farmerbob-b52)
                billing: Billing::Unknown,
                usd: match m {
                    Some((u, _)) => Measurement::observed(u),
                    None => Measurement::not_attempted(),
                },
                tokens: match m {
                    Some((_, t)) => Measurement::observed(t),
                    None => Measurement::not_attempted(),
                },
                arm,
                task,
            })
        })
        .collect())
}

/// Critic and prover spend, which lands in the SHARED opencode store.
///
/// fb_launch does not give critics and provers the per-run XDG_DATA_HOME that fb-dispatch
/// gives implementers, so their sessions all land in one store and carry no run identity --
/// only a model id. They are therefore attributed BY MODEL, which is coarser than the
/// per-run attribution above and is why they are summed separately and labelled.
///
/// It is not a rounding difference. This board reported $9.02 while the shell reported
/// $15.50, and the whole $5.53 gap was critic and prover work this function was not counting.
/// Two boards disagreeing by two thirds on the project's headline number is worse than one
/// board being wrong, because neither can be cited.  (bead farmerbob-jd2.9)
fn shared_store_spend(repo: &Path) -> BTreeMap<String, (f64, u64)> {
    let mut out: BTreeMap<String, (f64, u64)> = BTreeMap::new();
    let db = crate::paths::state()
        .parent()
        .map(|p| p.join("opencode/opencode.db"))
        .unwrap_or_default();
    if !db.exists() {
        return out;
    }
    // model id -> arm, from the registry. The session row carries the model, not the arm.
    let mut by_model: BTreeMap<String, String> = BTreeMap::new();
    if let Ok(body) = fs::read_to_string(repo.join("sources.toml"))
        && let Ok(doc) = toml::from_str::<toml::Value>(&body)
        && let Some(t) = doc.get("source").and_then(|s| s.as_table())
    {
        for (name, v) in t {
            if let Some(m) = v.get("model").and_then(|m| m.as_str()) {
                // Everything after the FIRST slash, not the last. The registry writes
                // "openrouter/thinkingmachines/inkling:free" and the session row carries
                // "thinkingmachines/inkling:free" -- so rsplit_once gives "inkling:free" and
                // matches nothing, which is how this returned $0.00 of shared spend against
                // the shell's $5.53 on its first run.
                let id = m.split_once('/').map_or(m, |(_, tail)| tail).to_string();
                by_model.insert(id, name.clone());
            }
        }
    }
    let Ok(conn) = rusqlite::Connection::open_with_flags(
        &db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    ) else {
        return out;
    };
    let Ok(mut stmt) = conn.prepare("select model, cost, tokens_input, tokens_output from session")
    else {
        return out;
    };
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, Option<String>>(0)?,
            r.get::<_, Option<f64>>(1)?,
            r.get::<_, Option<i64>>(2)?,
            r.get::<_, Option<i64>>(3)?,
        ))
    });
    let Ok(rows) = rows else { return out };
    for (model_json, cost, tin, tout) in rows.flatten() {
        let id = model_json
            .as_deref()
            .and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok())
            .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(str::to_string))
            .unwrap_or_default();
        let Some(arm) = by_model.get(&id) else {
            continue;
        };
        let e = out.entry(arm.clone()).or_insert((0.0, 0));
        e.0 += cost.unwrap_or(0.0);
        e.1 += u64::try_from(tin.unwrap_or(0)).unwrap_or(0)
            + u64::try_from(tout.unwrap_or(0)).unwrap_or(0);
    }
    out
}

/// Arms whose every counted run was unmeasured. They cannot sit on a cost frontier.
fn unpriced(runs: &[RunCost]) -> BTreeMap<String, (u32, u32)> {
    let mut seen: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    for r in runs.iter().filter(|r| r.counts_for_arm) {
        let e = seen.entry(r.arm.clone()).or_insert((0, 0));
        e.1 += 1;
        if r.usd.is_observed() {
            e.0 += 1;
        }
    }
    seen
}

// ---- Recovered tokens ---------------------------------------------------------------
//
// The TOKENS column used to read only the per-run opencode stores, so the arms whose
// launchers write no store showed `n/a` forever -- and the two arms with the most runs
// could not appear on the board at all. `cost::tokens_from_log` reads the count a launcher
// prints in its own log; the functions below bring that instrument to the board. They
// recover TOKENS and nothing else: a token count is not a dollar figure, and an arm whose
// price is unknown stays unpriced however completely its tokens are known.

/// The parts of `sources.toml` this module classifies launchers from.
///
/// A KNOWN SUBSET of the registry's arms: an arm is classified by its `cmd` key alone, and
/// only two values of that key are recognised (see [`launcher_for`]). Every other arm in
/// `sources.toml` -- the `launcher`-only opencode routes, arms with no `cmd` at all -- and
/// every arm absent from the file is [`Launcher::Unknown`], which is the safe direction: an
/// unrecognised launcher must not be assumed to use a log format it may not use.
///
/// This is deliberately not [`crate::sources::Registry`]. That type's `Source` never parsed
/// the `cmd` key, so it cannot answer the one question this module asks of the registry;
/// carrying the single column its callers need here keeps that type's twenty other fields
/// out of a module that would never read them.
#[derive(Debug)]
pub struct Registry {
    /// Per arm, the program `sources.toml` launches. Arms without a `cmd` key are absent.
    cmd: BTreeMap<String, String>,
}

impl Registry {
    /// Load the registry from a `sources.toml`.
    ///
    /// `Err` when the file cannot be read or is not valid TOML. A readable file with no
    /// `[source]` table is an empty registry, not an error: no arm is then classified,
    /// which makes every arm [`Launcher::Unknown`] rather than every arm broken.
    pub fn load(path: &Path) -> Result<Registry, String> {
        let text =
            fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let doc: toml::Value = toml::from_str(&text)
            .map_err(|e| format!("{} is not valid TOML: {e}", path.display()))?;
        let mut cmd = BTreeMap::new();
        if let Some(sources) = doc.get("source").and_then(|s| s.as_table()) {
            for (name, entry) in sources {
                if let Some(program) = entry.get("cmd").and_then(|c| c.as_str()) {
                    cmd.insert(name.clone(), program.to_string());
                }
            }
        }
        Ok(Registry { cmd })
    }
}

/// Which launcher wrote an arm's logs, from the registry's `cmd`.
///
/// The launcher behind an arm, from `farmerbob_core::cost::launcher_from_name`.
///
/// This was a PRIVATE duplicate of that table, knowing only `codex` and a `Zcode` variant
/// that no longer exists. Because production called this copy and not the shared one, the
/// Agy launcher added specifically to give agy arms a token count was unreachable and
/// changed nothing. Three critics found it independently -- two on launcher-agy, one on
/// cost-honesty -- which is evidence about the codebase rather than about any arm.
///   (bead farmerbob-lzae)
pub fn launcher_for(arm: &str, registry: &Registry) -> Launcher {
    match registry.cmd.get(arm).map(String::as_str) {
        Some(cmd) => cost::launcher_from_name(cmd),
        None => cost::launcher_from_name(arm),
    }
}

/// Tokens recovered from an arm's own run logs, per run, summed.
///
/// The logs live in the harness logs directory, one per run, named `<task>--<arm>.log`.
/// The arm's launcher is classified from the registry beside the repository
/// (`sources.toml` as [`crate::paths::repo`] resolves it); when that registry cannot be
/// read, the launcher cannot be classified, and the honest answer is
/// [`Measurement::Missing`] -- never a parse of the logs under an assumed format.
///
/// Runs whose log reports no count -- including logs that cannot be read at all -- do not
/// contribute, so when SOME runs report, the sum is over the reporting runs and is a lower
/// bound on the arm's total. That is the same accounting
/// [`farmerbob_core::cost::aggregate`] applies to store-measured tokens: the runs that did
/// report carry real measurements, and discarding them because a sibling run was silent
/// would trade a stated lower bound for an absence. The board keeps the distinction where
/// it keeps all of them: an arm whose TOKENS rest on a partial recovery is still unpriced
/// until a price exists. It is [`Measurement::Missing`] when no run of that arm reported a
/// count -- which is NOT zero tokens, and must never be rendered as `0`.
pub fn recovered_tokens(logs_dir: &Path, arm: &str) -> Measurement<u64> {
    let registry_path = crate::paths::repo().join("sources.toml");
    match Registry::load(&registry_path) {
        Ok(registry) => recovered_tokens_for(logs_dir, arm, &registry),
        Err(e) => Measurement::instrument_failed(&format!(
            "cannot classify the launcher of arm '{arm}': {e}"
        )),
    }
}

/// [`recovered_tokens`] against a caller-supplied registry, so classification and summing
/// can be exercised against fixture logs and a fixture registry instead of the
/// repository's own `sources.toml`.
fn recovered_tokens_for(logs_dir: &Path, arm: &str, registry: &Registry) -> Measurement<u64> {
    sum_run_logs(logs_dir, arm, launcher_for(arm, registry))
}

/// Sum [`tokens_from_log`] over every log of one arm, under one launcher.
fn sum_run_logs(logs_dir: &Path, arm: &str, launcher: Launcher) -> Measurement<u64> {
    if launcher == Launcher::Unknown {
        return Measurement::nothing_to_measure(&format!(
            "launcher of arm '{arm}' is unrecognised, so its logs are not read: \
             what an unknown launcher counts is unknown"
        ));
    }
    let entries = match fs::read_dir(logs_dir) {
        Ok(entries) => entries,
        Err(e) => {
            return Measurement::instrument_failed(&format!(
                "cannot read logs directory {}: {e}",
                logs_dir.display()
            ));
        }
    };
    let suffix = format!("--{arm}.log");
    let mut logs: Vec<std::path::PathBuf> = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        if entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.ends_with(&suffix))
        {
            logs.push(entry.path());
        }
    }
    if logs.is_empty() {
        return Measurement::nothing_to_measure(&format!(
            "no run log named <task>--{arm}.log in {}: no run of that arm reported a count",
            logs_dir.display()
        ));
    }
    let mut reported = 0usize;
    let mut total: u64 = 0;
    for log in &logs {
        // A log that cannot be read is a run that did not report, not a run that reported
        // nothing; the reason on the Missing below is what says how many logs were seen.
        let Ok(body) = fs::read_to_string(log) else {
            continue;
        };
        // tokens_from_log now reports three dimensions; the curve wants what was billed as
        // prompt plus completion. A Missing dimension contributes nothing and does not
        // suppress the other -- they answer different questions.
        let t = tokens_from_log(launcher, &body);
        // Dimensions when the launcher reports them, otherwise the single figure it does
        // report. Never both: `combined` is not a total of the three and adding it would
        // double count.
        let summed: u64 = if t.input.is_observed() || t.output.is_observed() {
            t.input
                .value()
                .copied()
                .unwrap_or(0)
                .saturating_add(t.output.value().copied().unwrap_or(0))
        } else {
            t.combined.value().copied().unwrap_or(0)
        };
        if t.input.is_observed() || t.output.is_observed() || t.combined.is_observed() {
            reported += 1;
            total = total.saturating_add(summed);
        }
    }
    if reported == 0 {
        Measurement::nothing_to_measure(&format!(
            "{} run log(s) of arm '{arm}' in {} reported no token count",
            logs.len(),
            logs_dir.display()
        ))
    } else {
        Measurement::observed(total)
    }
}

pub fn run_cmd(epsilon: f64, json_only: bool) -> i32 {
    let base = match std::env::var("HOME") {
        Ok(_) => crate::paths::state(),
        Err(_) => {
            eprintln!("error: HOME is not set");
            return 2;
        }
    };
    let runs = match load_runs(&base) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if runs.is_empty() {
        eprintln!("no scored runs in logs/objective.json");
        return 3;
    }

    let arms: Vec<ArmCost> = aggregate(&runs);
    // Tokens for arms the opencode stores never measured, recovered from each launcher's own
    // run log. Deliberately a SIDE DISPLAY and nothing more: nothing recovered here enters
    // `runs` or `arms`, so it cannot reach the frontier, the totals or any dollar figure.
    // Computing the frontier membership on `arms` alone, before and after this recovery
    // existed, gives the same members -- that is pinned, and staying true is what keeps an
    // arm with no dollar figure off a cost curve.
    let recovered: BTreeMap<String, Measurement<u64>> = arms
        .iter()
        .filter(|a| !a.tokens.is_observed())
        .map(|a| {
            (
                a.arm.clone(),
                recovered_tokens(&crate::paths::logs(), &a.arm),
            )
        })
        .collect();
    // UNPRICED counts runs with no DOLLAR figure and nothing else. Recovered tokens do not
    // touch it: a known token total is not a known price, and an arm whose tokens just
    // appeared on this board must not read as if its bill had appeared with them.
    let priced = unpriced(&runs);
    // `frontier` now excludes a partially measured arm itself: ArmCost carries
    // `unmeasured_runs` and `cost::fully_measured` reads it. This used to pre-filter the field
    // here, which was a caller compensating for a type that could not express what it knew.
    // Two critics pointed out the duplication survives as drift risk if it is left in place --
    // "any divergence between the two mechanisms will produce silent double-filtering or
    // unfiltered partial arms reaching the frontier" -- so it is gone rather than merely
    // redundant.  (bead farmerbob-jd2.9)
    let front = frontier(&arms, epsilon);
    let (spend, completed, counted) = totals(&arms);
    // Fold in critic and prover spend, kept separate in the report because it is attributed
    // by MODEL rather than per run and must never be mistaken for per-run attribution.
    let shared = shared_store_spend(&crate::paths::repo());
    let shared_total: f64 = shared.values().map(|(c, _)| c).sum();
    // Money spent on runs that do NOT count as arm results -- infrastructure faults, quota
    // refusals, orchestrator kills. It is real spend and belongs in a total; it is not
    // attributable to any arm's capability and must never enter a posterior or a $/success.
    // fb-pareto.sh folds it into its total silently, which is most of why that board reads
    // $15.50 where this one reads $14.55.
    let excluded_spend: f64 = runs
        .iter()
        .filter(|r| !r.counts_for_arm)
        .filter_map(|r| r.usd.value().copied())
        .sum();

    if json_only {
        let rows: Vec<serde_json::Value> = arms
            .iter()
            .map(|a| {
                let (p, n) = priced.get(&a.arm).copied().unwrap_or((0, 0));
                serde_json::json!({
                    "arm": a.arm,
                    "canonical": canonical_model(&a.arm),
                    "runs": a.runs,
                    "completed": a.completed,
                    "completion_rate": a.completion_rate().value().copied(),
                    "usd": a.usd.value(),
                    "usd_per_completion": a.usd_per_completion().value().copied(),
                    "tokens": a.tokens.value(),
                    // Recovered from the launcher's own logs where the store measured
                    // nothing; null means no run of this arm reported a count, which is
                    // not zero. Kept beside `tokens` rather than folded into it: the two
                    // figures come from different instruments.
                    "recovered_tokens": recovered.get(&a.arm).and_then(|m| m.value().copied()),
                    "priced_runs": p,
                    "unpriced_runs": n.saturating_sub(p),
                    "on_frontier": front.contains(&a.arm),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "arms": rows, "frontier": front,
                "measured_spend": spend.value().copied(), "completed": completed, "counted_runs": counted,
            }))
            .unwrap_or_else(|_| "{}".into())
        );
        return 0;
    }

    println!(
        "{:<24}{:>10}{:>5}{:>11}{:>12}{:>11}{:>10}  FRONTIER",
        "ARM", "COMPLETE", "N", "TOTAL $", "$/SUCCESS", "TOKENS", "UNPRICED"
    );
    println!("{}", "-".repeat(94));
    let mut sorted = arms.clone();
    sorted.sort_by(|a, b| {
        b.completion_rate()
            .value()
            .copied()
            .unwrap_or(0.0)
            .partial_cmp(&a.completion_rate().value().copied().unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.arm.cmp(&b.arm))
    });
    for a in &sorted {
        if a.runs == 0 {
            continue;
        }
        let (p, n) = priced.get(&a.arm).copied().unwrap_or((0, 0));
        // Never print a dollar figure for an arm nothing measured. "$0.0000" is a claim.
        let usd = match a.usd.value() {
            Some(v) => format!("{v:.4}"),
            None => "n/a".into(),
        };
        // usd_per_completion now returns None itself when any counted run was unmeasured.
        let per = match a.usd_per_completion().value() {
            Some(v) => format!("{v:.4}"),
            None => "n/a".into(),
        };
        let rate = match a.completion_rate().value() {
            Some(r) => format!("{:.0}%", r * 100.0),
            None => "n/a".into(),
        };
        let unp = if n > p {
            format!("{}/{}", n - p, n)
        } else {
            "-".into()
        };
        // The store figure when there is one; otherwise the recovery from the launcher's own
        // logs. `n/a` here covers both "no store measured this" and "no log reported a
        // count" -- Missing, never 0: zero tokens is a claim, absence is absence.
        let tokens = match a
            .tokens
            .value()
            .copied()
            .or_else(|| recovered.get(&a.arm).and_then(|m| m.value().copied()))
        {
            Some(t) => t.to_string(),
            None => "n/a".into(),
        };
        let mark = if front.contains(&a.arm) {
            "  <= pareto"
        } else {
            ""
        };
        println!(
            "{:<24}{rate:>10}{:>5}{usd:>11}{per:>12}{tokens:>11}{unp:>10}{mark}",
            a.arm, a.runs,
        );
    }

    let never: Vec<&str> = priced
        .iter()
        .filter(|(_, (p, n))| *n > 0 && *p == 0)
        .map(|(k, _)| k.as_str())
        .collect();
    if !never.is_empty() {
        println!(
            "\n  NOT PRICED AT ALL, and therefore NOT on the frontier: {}",
            never.join(", ")
        );
        println!("  Their launchers write no cost store. Absence of a bill is not a bill of zero.");
    }
    if shared_total > 0.0 {
        println!(
            "\n  ${shared_total:.2} of critic and prover spend from the shared store, \
             attributed by model across {} arm(s)",
            shared.len()
        );
        println!("  It is coarser than the per-run figures above and is summed separately.");
    }
    match spend.value().copied() {
        Some(s) => {
            println!(
                "  {counted} counted runs, {completed} completed, ${:.4} attributable spend \
                 (${s:.4} per-run + ${shared_total:.4} shared)",
                s + shared_total
            );
            if excluded_spend > 0.0 {
                println!(
                    "  ${excluded_spend:.4} more was spent on runs that do not count as arm \
                     results, for ${:.4} total.",
                    s + shared_total + excluded_spend
                );
                println!(
                    "  That is real money and belongs in a total; it is not evidence about \
                     any arm, so it stays out of every per-arm figure above."
                );
            }
        }
        None if shared_total > 0.0 => println!(
            "  {counted} counted runs, {completed} completed, per-run spend UNMEASURED, \
             ${shared_total:.4} shared"
        ),
        None => println!("  {counted} counted runs, {completed} completed, spend UNMEASURED"),
    }
    0
}

#[cfg(test)]
mod recovered_token_tests {
    use super::*;

    /// A private sandbox under the system temp dir: no test reads the real logs directory
    /// or the real `sources.toml`. Unique `tag` per test, so parallel tests never share one.
    fn sandbox(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fb-pareto-recovered-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("test sandbox should be creatable");
        dir
    }

    /// One registry fixture for every test: the recognised subset (`codex`, `zcode`) and
    /// the arms that must stay unrecognised (another CLI, a `launcher`-only route).
    fn fixture_registry(tag: &str) -> Registry {
        let dir = sandbox(tag);
        fs::write(
            dir.join("sources.toml"),
            "[source.codex-arm]\ncmd = \"codex\"\n\
             [source.zcode-arm]\ncmd = \"zcode\"\n\
             [source.claude-arm]\ncmd = \"claude\"\n\
             [source.ori-opencode-arm]\nlauncher = \"ori opencode\"\n\
             [source.long-arm-name]\ncmd = \"codex\"\n",
        )
        .expect("fixture sources.toml should be writable");
        Registry::load(&dir.join("sources.toml")).expect("fixture registry should parse")
    }

    fn write_log(dir: &Path, task: &str, arm: &str, body: &str) {
        fs::write(dir.join(format!("{task}--{arm}.log")), body)
            .expect("fixture log should be writable");
    }

    fn codex_report(count: &str) -> String {
        format!("exec 41s\nstream closed\ntokens used\n{count}\n")
    }

    // ---- launcher_for: the known subset, and nothing more ------------------------------

    // The registry's `cmd` classifies exactly two launchers.
    #[test]
    fn the_known_cmds_map_to_the_known_launchers() {
        let registry = fixture_registry("cmds");
        assert_eq!(launcher_for("codex-arm", &registry), Launcher::Codex);
        // `zcode` was a variant of the PRIVATE table this file used to keep. The shared
        // table in cost.rs does not know it, and Unknown is the honest answer for a launcher
        // nothing models -- a guess here misattributes an arm's whole bill.
        assert_eq!(launcher_for("zcode-arm", &registry), Launcher::Unknown);
    }

    // A cmd the SHARED table models is now recognised; everything genuinely unmodelled is
    // still Unknown. `claude` asserted Unknown here only because the private table this file
    // used to keep had never heard of it -- the test encoded that table's ignorance, which is
    // the bug, not the contract.
    #[test]
    fn every_other_cmd_and_arm_is_unknown() {
        let registry = fixture_registry("unknowns");
        assert_eq!(launcher_for("claude-arm", &registry), Launcher::Claude);
        assert_eq!(
            launcher_for("ori-opencode-arm", &registry),
            Launcher::Unknown
        );
        assert_eq!(launcher_for("no-such-arm", &registry), Launcher::Unknown);
    }

    // A readable sources.toml with no [source] table classifies nothing; that is an empty
    // registry, not a broken one.
    #[test]
    fn a_sourceless_toml_is_an_empty_registry_not_an_error() {
        let dir = sandbox("sourceless");
        fs::write(dir.join("sources.toml"), "# nothing here\n")
            .expect("fixture should be writable");
        let registry = Registry::load(&dir.join("sources.toml")).expect("should parse");
        assert_eq!(launcher_for("codex-arm", &registry), Launcher::Unknown);
    }

    #[test]
    fn an_unreadable_registry_is_an_error_naming_the_file() {
        let dir = sandbox("unreadable-registry");
        let err = Registry::load(&dir.join("missing").join("sources.toml"))
            .expect_err("a missing file cannot load");
        assert!(err.contains("cannot read"), "reason was: {err}");
    }

    #[test]
    fn an_unparsable_registry_is_an_error() {
        let dir = sandbox("bad-toml");
        fs::write(dir.join("sources.toml"), "[sourceBroken").expect("fixture should be writable");
        assert!(Registry::load(&dir.join("sources.toml")).is_err());
    }

    // ---- recovered_tokens: the clauses -------------------------------------------------

    // Clause 1: an arm whose logs carry counts shows their sum. Logs of other arms and
    // files that are not run logs are not the arm's runs and contribute nothing.
    #[test]
    fn an_arms_logs_sum_over_its_own_runs_only() {
        let dir = sandbox("sum");
        let registry = fixture_registry("sum");
        write_log(&dir, "budget", "codex-arm", &codex_report("130,826"));
        write_log(&dir, "queue", "codex-arm", &codex_report("53,557"));
        write_log(&dir, "budget", "zcode-arm", &codex_report("999"));
        fs::write(dir.join("budget--codex-arm.json"), "{}").expect("fixture should be writable");
        let got = recovered_tokens_for(&dir, "codex-arm", &registry);
        assert_eq!(got, Measurement::Observed(184_383));
    }

    // Boundary: one arm, one run, one count -- that count.
    #[test]
    fn one_run_with_one_count_is_that_count() {
        let dir = sandbox("one-run");
        let registry = fixture_registry("one-run");
        write_log(&dir, "budget", "codex-arm", &codex_report("7"));
        assert_eq!(
            recovered_tokens_for(&dir, "codex-arm", &registry),
            Measurement::Observed(7)
        );
    }

    // Boundary: many runs, all reporting, is the sum of all of them -- the shape the real
    // codex-luna arm presents (46 logs, 46 reports).
    #[test]
    fn forty_six_reporting_runs_sum_all_forty_six() {
        let dir = sandbox("forty-six");
        let registry = fixture_registry("forty-six");
        for n in 0..46 {
            write_log(
                &dir,
                &format!("task-{n}"),
                "codex-arm",
                &codex_report("1000"),
            );
        }
        assert_eq!(
            recovered_tokens_for(&dir, "codex-arm", &registry),
            Measurement::Observed(46_000)
        );
    }

    // Boundary, pinned: when SOME runs report and some do not, the sum is over the
    // reporting runs -- a lower bound, per the doc comment and cost::aggregate's rule for
    // store-measured tokens. A silent sibling does not erase measured siblings.
    #[test]
    fn some_runs_silent_sum_over_the_reporting_ones() {
        let dir = sandbox("some-silent");
        let registry = fixture_registry("some-silent");
        write_log(&dir, "reported", "codex-arm", &codex_report("4096"));
        write_log(&dir, "silent", "codex-arm", "exec 41s\nstream closed\n");
        assert_eq!(
            recovered_tokens_for(&dir, "codex-arm", &registry),
            Measurement::Observed(4_096)
        );
    }

    // ...and when NO run reports, the whole measurement is Missing, never a zero.
    #[test]
    fn logs_without_any_report_are_missing_not_zero() {
        let dir = sandbox("all-silent");
        let registry = fixture_registry("all-silent");
        write_log(&dir, "silent", "codex-arm", "exec 41s\nstream closed\n");
        write_log(&dir, "silent-too", "codex-arm", &codex_report("n/a"));
        assert!(matches!(
            recovered_tokens_for(&dir, "codex-arm", &registry),
            Measurement::Missing(_)
        ));
    }

    // Clause 3: an arm with no logs at all is Missing -- nothing to measure, which is a
    // different fact from an instrument that could not run.
    #[test]
    fn an_arm_with_no_logs_is_nothing_to_measure() {
        let dir = sandbox("no-logs");
        let registry = fixture_registry("no-logs");
        assert!(matches!(
            recovered_tokens_for(&dir, "codex-arm", &registry),
            Measurement::Missing(_)
        ));
    }

    // Clause 3's other edge: a logs directory that cannot be read is Missing with a
    // reason -- never 0, and a different reason from "no logs".
    #[test]
    fn an_unreadable_logs_directory_is_instrument_failed() {
        let registry = fixture_registry("unreadable-logs");
        let missing = sandbox("unreadable-logs-root").join("does-not-exist");
        match recovered_tokens_for(&missing, "codex-arm", &registry) {
            Measurement::Missing(reason) => {
                assert!(
                    matches!(
                        reason,
                        farmerbob_core::measurement::Absent::InstrumentFailed { .. }
                    ),
                    "expected an instrument failure, got {reason:?}"
                );
            }
            other => panic!("expected Missing, got {other:?}"),
        }
    }

    // Clause 4: an unrecognised launcher yields Missing even when its logs contain the
    // marker -- the logs are not read at all.
    #[test]
    fn an_unknown_launcher_is_missing_even_with_marker_logs() {
        let dir = sandbox("unknown-launcher");
        let registry = fixture_registry("unknown-launcher");
        write_log(&dir, "budget", "claude-arm", &codex_report("130,826"));
        assert!(matches!(
            recovered_tokens_for(&dir, "claude-arm", &registry),
            Measurement::Missing(_)
        ));
        assert!(matches!(
            recovered_tokens_for(&dir, "ori-opencode-arm", &registry),
            Measurement::Missing(_)
        ));
    }

    // A zcode arm is recognised but its launcher reports no counts, so recovery is Missing
    // on logs that genuinely carry none.
    #[test]
    fn a_zcode_arm_is_recognised_but_still_reports_nothing() {
        let dir = sandbox("zcode-arm");
        let registry = fixture_registry("zcode-arm");
        write_log(&dir, "budget", "zcode-arm", "step 1 done\nstep 2 done\n");
        assert!(matches!(
            recovered_tokens_for(&dir, "zcode-arm", &registry),
            Measurement::Missing(_)
        ));
    }

    // Clause 5: one measured zero among measured values is a value.
    #[test]
    fn a_reported_zero_is_observed_zero() {
        let dir = sandbox("zero-alone");
        let registry = fixture_registry("zero-alone");
        write_log(&dir, "budget", "codex-arm", &codex_report("0"));
        assert_eq!(
            recovered_tokens_for(&dir, "codex-arm", &registry),
            Measurement::Observed(0)
        );
    }

    // Clause 5 again: a zero CONTRIBUTES zero and does not turn a sum into Missing.
    #[test]
    fn a_zero_among_values_contributes_zero() {
        let dir = sandbox("zero-mixed");
        let registry = fixture_registry("zero-mixed");
        write_log(&dir, "a", "codex-arm", &codex_report("0"));
        write_log(&dir, "b", "codex-arm", &codex_report("4096"));
        assert_eq!(
            recovered_tokens_for(&dir, "codex-arm", &registry),
            Measurement::Observed(4_096)
        );
    }

    // Clause 6: summing saturates at u64::MAX rather than wrapping.
    #[test]
    fn sums_saturate_at_u64_max() {
        let dir = sandbox("saturate");
        let registry = fixture_registry("saturate");
        let max = "18,446,744,073,709,551,615";
        write_log(&dir, "a", "codex-arm", &codex_report(max));
        write_log(&dir, "b", "codex-arm", &codex_report(max));
        assert_eq!(
            recovered_tokens_for(&dir, "codex-arm", &registry),
            Measurement::Observed(u64::MAX)
        );
    }

    // The `<task>--<arm>.log` suffix is a whole-name boundary: an arm whose name is a
    // suffix of another arm's name does not inherit that arm's logs.
    #[test]
    fn an_arm_does_not_inherit_a_longer_named_arms_logs() {
        let dir = sandbox("suffix");
        let registry = fixture_registry("suffix");
        write_log(&dir, "task", "long-arm-name", &codex_report("4096"));
        assert_eq!(
            recovered_tokens_for(&dir, "long-arm-name", &registry),
            Measurement::Observed(4_096)
        );
        assert!(matches!(
            recovered_tokens_for(&dir, "name", &registry),
            Measurement::Missing(_)
        ));
    }

    // A directory that happens to be named like a log is not a log.
    #[test]
    fn a_directory_named_like_a_log_is_not_a_run() {
        let dir = sandbox("dir-log");
        let registry = fixture_registry("dir-log");
        fs::create_dir_all(dir.join("task--codex-arm.log"))
            .expect("fixture dir should be creatable");
        assert!(matches!(
            recovered_tokens_for(&dir, "codex-arm", &registry),
            Measurement::Missing(_)
        ));
    }

    // The public entry point, against the repository's own registry: an arm no registry
    // names is Unknown, and Unknown is Missing -- whatever the environment resolves
    // `sources.toml` to, and whether or not it can be read.
    #[test]
    fn the_public_entry_point_is_missing_for_an_unregistered_arm() {
        let dir = sandbox("public");
        write_log(
            &dir,
            "budget",
            "no-such-arm-anywhere",
            &codex_report("130,826"),
        );
        assert!(matches!(
            recovered_tokens(&dir, "no-such-arm-anywhere"),
            Measurement::Missing(_)
        ));
    }

    // ---- the pins that keep recovery in its lane ----------------------------------------

    // Clause 2: the UNPRICED column counts runs with no DOLLAR figure. Tokens -- recovered
    // or store-measured -- do not price anything.
    #[test]
    fn tokens_known_and_bill_absent_is_still_wholly_unpriced() {
        let runs = [RunCost {
            arm: "codex-luna".into(),
            task: "t".into(),
            billing: Billing::Unknown,
            usd: Measurement::not_attempted(),
            tokens: Measurement::observed(53_557),
            completed: true,
            counts_for_arm: true,
        }];
        assert_eq!(
            unpriced(&runs).get("codex-luna"),
            Some(&(0, 1)),
            "a token total is not a bill: the run stays unpriced"
        );
    }

    // Clause 7: recovering tokens must not move the frontier. Membership is computed on
    // the same arms with and without token figures, and the unpriced arm stays off the
    // curve even holding the largest token count on the board.
    #[test]
    fn recovered_tokens_do_not_move_the_frontier() {
        fn arm_of(name: &str, usd: Option<f64>, tokens: Option<u64>) -> ArmCost {
            ArmCost {
                arm: name.into(),
                runs: 2,
                completed: 2,
                // A priced arm is Metered, not Free: `fully_measured` accepts a zero only
                // from a billing whose zero is real, and Free would make an unpriced arm
                // look measured -- the exact substitution this task removed.
                billing: usd.map_or(Billing::Unknown, |_| Billing::Metered {
                    input_per_mtok: 1.0,
                    output_per_mtok: 1.0,
                }),
                usd: usd.map_or(Measurement::not_attempted(), Measurement::observed),
                tokens: tokens.map_or(Measurement::not_attempted(), Measurement::observed),
                unmeasured_runs: 0,
            }
        }
        let arms = vec![
            arm_of("codex-luna", None, Some(9_000_000)),
            arm_of("priced", Some(1.0), None),
        ];
        let with_tokens = frontier(&arms, 0.0);
        let stripped: Vec<ArmCost> = arms
            .iter()
            .map(|a| ArmCost {
                tokens: Measurement::not_attempted(),
                ..a.clone()
            })
            .collect();
        assert_eq!(
            with_tokens,
            frontier(&stripped, 0.0),
            "tokens are not an input to the frontier, before or after recovery"
        );
        assert_eq!(
            with_tokens,
            vec!["priced".to_string()],
            "an arm with no dollar figure stays off the curve"
        );
    }
}
