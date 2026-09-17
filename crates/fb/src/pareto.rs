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

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use farmerbob_core::cost::{aggregate, frontier, totals, ArmCost, RunCost};
use farmerbob_core::pricing::canonical_model;

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
    let body = fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(&body).map_err(|e| format!("{} is not valid JSON: {e}", path.display()))?;

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
                usd: m.map(|(u, _)| u),
                tokens: m.map(|(_, t)| t),
                arm,
                task,
            })
        })
        .collect())
}

/// Arms whose every counted run was unmeasured. They cannot sit on a cost frontier.
fn unpriced(runs: &[RunCost]) -> BTreeMap<String, (u32, u32)> {
    let mut seen: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    for r in runs.iter().filter(|r| r.counts_for_arm) {
        let e = seen.entry(r.arm.clone()).or_insert((0, 0));
        e.1 += 1;
        if r.usd.is_some() {
            e.0 += 1;
        }
    }
    seen
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
                    "completion_rate": a.completion_rate(),
                    "usd": a.usd,
                    "usd_per_completion": a.usd_per_completion(),
                    "tokens": a.tokens,
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
                "measured_spend": spend, "completed": completed, "counted_runs": counted,
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
            .unwrap_or(0.0)
            .partial_cmp(&a.completion_rate().unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.arm.cmp(&b.arm))
    });
    for a in &sorted {
        if a.runs == 0 {
            continue;
        }
        let (p, n) = priced.get(&a.arm).copied().unwrap_or((0, 0));
        // Never print a dollar figure for an arm nothing measured. "$0.0000" is a claim.
        let usd = a.usd.map(|v| format!("{v:.4}")).unwrap_or_else(|| "n/a".into());
        // usd_per_completion now returns None itself when any counted run was unmeasured.
        let per = a.usd_per_completion().map(|v| format!("{v:.4}")).unwrap_or_else(|| "n/a".into());
        let rate = a.completion_rate().map(|r| format!("{:.0}%", r * 100.0)).unwrap_or_else(|| "n/a".into());
        let unp = if n > p { format!("{}/{}", n - p, n) } else { "-".into() };
        let mark = if front.contains(&a.arm) { "  <= pareto" } else { "" };
        println!(
            "{:<24}{rate:>10}{:>5}{usd:>11}{per:>12}{:>11}{unp:>10}{mark}",
            a.arm,
            a.runs,
            a.tokens.map(|t| t.to_string()).unwrap_or_else(|| "n/a".into()),
        );
    }

    let never: Vec<&str> = priced
        .iter()
        .filter(|(_, (p, n))| *n > 0 && *p == 0)
        .map(|(k, _)| k.as_str())
        .collect();
    if !never.is_empty() {
        println!("\n  NOT PRICED AT ALL, and therefore NOT on the frontier: {}", never.join(", "));
        println!("  Their launchers write no cost store. Absence of a bill is not a bill of zero.");
    }
    match spend {
        Some(s) => println!("\n  {counted} counted runs, {completed} completed, ${s:.4} measured spend"),
        None => println!("\n  {counted} counted runs, {completed} completed, spend UNMEASURED"),
    }
    0
}
