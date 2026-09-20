//! `fb park` — the first caller of the quota chain.
//!
//! `park_after`, `blast_of`, `arms_in_blast`, `decide` and `grounds` were specified, critiqued
//! and merged across five waves. Nothing called any of them, which is why the thing that
//! motivated them kept happening: thirteen dispatch slots in one day spent on arms that refused
//! before they started, and five free arms eligible and undispatched for 28.8 hours behind a
//! park whose reset instant had been invented by hand.
//!
//! This reads a finished run's log, classifies it, and asks core what to do. Every rule lives
//! in core and is tested there. What lives here is I/O, and the refusal to act without
//! `--apply`.
//!
//! It does NOT invent a reset. `park_after` returns `Backoff` when the provider states no
//! instant, and the `grounds` string that survives into the decision is the provider's own
//! words -- which is the whole reason `park_grounds` added a field for it.

use farmerbob_core::limit_signal::SignalRules;
use farmerbob_core::outcome::{RunFacts, classify};
use farmerbob_core::park_decision::{Decision, decide, grounds};
use farmerbob_core::quota::{Blast, Registry, Source};
use std::collections::BTreeMap;

/// Build the core registry from `sources.toml`.
///
/// `quota::Registry` and `fb::sources::Registry` are two types over one file --
/// farmerbob's own N-of-N finding, filed when every arm of `park-scope` invented
/// a registry because the spec named one without saying where it lived. Until
/// that is resolved, this is the projection, in one place, named for what it is.
fn core_registry(reg: &crate::sources::Registry) -> Registry {
    let mut sources = BTreeMap::new();
    for (name, s) in &reg.sources {
        sources.insert(
            name.clone(),
            Source {
                provider: s.provider.clone(),
                quota_bucket: s.quota_bucket.clone(),
            },
        );
    }
    Registry { sources }
}

/// The exit status a log records, when the run left no run record.
///
/// A critic writes no run json, so `run_facts` finds nothing and `fb park` refuses -- which
/// is right in general: "a run with no recorded facts is not a run that succeeded, and
/// guessing would park arms on evidence nobody gathered."
///
/// But the exit code is not a guess here. `launch_in` records it in the log itself:
///
///     InstrumentFailed { reason: "agy-sonnet-46 exited with exit status: 3: ..." }
///
/// and the code is load-bearing evidence, not a formality: `limit_signal::classify` returns
/// `Ambiguous` for a refusal-shaped phrase seen with exit code 0, precisely so a caller
/// cannot park on wording alone. Reading the recorded status is using evidence the harness
/// gathered; inventing one would be the thing the refusal exists to prevent.
///
/// (bead farmerbob-4uha: agy-opus-46 and agy-sonnet-46 each burned a critic dispatch on the
/// same account-wide quota on 2026-09-19, twelve minutes apart, and neither was parked.)
pub fn exit_code_from_log(head: &str) -> Option<i32> {
    const MARKER: &str = "exited with exit status: ";
    let at = head.find(MARKER)? + MARKER.len();
    let digits: String = head[at..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// Where a run's log lives, for either kind of run.
///
/// An implementer writes `<task>--<arm>.log` at the logs root; a CRITIC writes
/// `critiques/<task>/<arm>.log`. This resolved only the first, so `fb park <task> <critic>`
/// answered "no log ... nothing to classify" -- the quota chain could not classify a critic
/// run even when called by hand at the right moment with the right arguments.
///
/// Not hypothetical: agy-opus-46 hit a provider quota AS A CRITIC twice on 2026-09-19, and
/// the park in `sources.toml` carries a hand-written note reading "429 as a CRITIC, not as
/// an implementer" -- a human working around this.  (bead farmerbob-4uha)
///
/// The implementer path is tried FIRST, so nothing about existing callers changes.
pub fn log_paths(logs: &std::path::Path, task: &str, arm: &str) -> Vec<std::path::PathBuf> {
    vec![
        logs.join(format!("{task}--{arm}.log")),
        logs.join("critiques").join(task).join(format!("{arm}.log")),
    ]
}

/// The first bytes of a run's log, where a refusal is stated if it is stated at all.
fn log_head(logs: &std::path::Path, task: &str, arm: &str) -> Option<String> {
    for p in log_paths(logs, task, arm) {
        if let Ok(text) = std::fs::read_to_string(&p) {
            return Some(text.chars().take(4000).collect());
        }
    }
    None
}

/// The facts a run recorded, as `outcome::classify` wants them.
///
/// The run json carries `rc`, `lines_added` and a `verdict`; it does NOT carry an
/// outcome class. An earlier version of this file read `outcome_class` and refused
/// on every run in the store, because the field does not exist -- an honest refusal
/// that made the command useless. The classification lives in core and is called
/// here rather than duplicated.
///
/// `declared` stays `None`: nothing in the pipeline writes an authoritative class
/// today, and inventing one here would override the very rules being called.
fn run_facts(logs: &std::path::Path, task: &str, arm: &str) -> Option<RunFacts> {
    let text = std::fs::read_to_string(logs.join(format!("{task}--{arm}.json"))).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(RunFacts {
        exit_code: v.get("rc").and_then(|c| c.as_i64()).map(|c| c as i32),
        lines_added: v
            .get("lines_added")
            .and_then(|c| c.as_u64())
            .map(|c| c as u32),
        log_head: log_head(logs, task, arm),
        declared: None,
    })
}

/// The refusal formats this project has actually observed, as data.
///
/// A known subset and not a closed set, which is why `SignalRules` takes patterns
/// rather than hard-coding them. Every string here was copied from a real log in
/// this repository's history.
fn rules(default_backoff_secs: u64) -> SignalRules {
    // LINE-START ANCHORED. `line_starts_with_a_limit_pattern` requires the log line to BEGIN
    // with the pattern, which is deliberate: an arm that QUOTES a refusal in its own prose
    // must not be read as having been refused, and that false positive has bitten this
    // project before. So every pattern here carries the `error: ` prefix the launchers
    // actually emit. A first draft dropped it and matched nothing at all -- every real
    // refusal in the store classified as Infrastructure via the shape rule instead, which
    // is the right answer to the wrong question.
    //
    // Copied from fb-objective.sh, which curated them from real logs, so the two readers of
    // the same evidence cannot disagree.
    SignalRules::new(default_backoff_secs)
        .with_pattern("error: individual quota reached")
        .with_pattern("error: quota exceeded")
        .with_pattern("error: rate limit exceeded")
        .with_pattern("error: 429")
        .with_pattern("usage limit reached")
        .with_exit_code(429)
}

fn blast_word(b: Blast) -> &'static str {
    match b {
        Blast::Arm => "this arm only",
        Blast::Bucket => "every arm on this vendor bucket",
        Blast::Credential => "every arm on this credential",
    }
}

/// An epoch-millisecond instant as RFC3339 UTC, without pulling in a date crate
/// this binary does not otherwise use. Days-from-civil, the standard algorithm.
fn format_instant(ms: u64) -> String {
    let secs = ms / 1000;
    let (mut days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    days += 719_468;
    let era = days.div_euclid(146_097);
    let doe = days.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}+00:00")
}

/// Decide, print, and with `apply`, write `parked_until` into `sources.toml`.
pub fn run_cmd(task: &str, arm: &str, apply: bool, default_backoff_secs: u64) -> i32 {
    let logs = crate::paths::logs();
    let reg_path = crate::paths::repo().join("sources.toml");
    let reg = match crate::sources::Registry::load(&reg_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("fb park: reading the registry: {e}");
            return 1;
        }
    };

    let Some(head) = log_head(&logs, task, arm) else {
        eprintln!(
            "fb park: no log for {task}/{arm}; nothing to classify. Looked in: {}",
            log_paths(&logs, task, arm)
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        return 2;
    };
    let class = match run_facts(&logs, task, arm) {
        Some(facts) => classify(&facts, &rules(default_backoff_secs)),
        None => {
            // No run record -- a critic writes none. The log may still carry the exit
            // status the launcher recorded, and that is evidence, not a guess.
            let Some(code) = exit_code_from_log(&head) else {
                // Refusing is the point. A run with no recorded facts and no recorded exit
                // status is not a run that succeeded, and guessing would park arms on
                // evidence nobody gathered.
                eprintln!(
                    "fb park: no run record for {task}--{arm}, and its log records no exit \
                     status; refusing to guess"
                );
                return 2;
            };
            let now_s = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            match farmerbob_core::limit_signal::classify(
                &rules(default_backoff_secs),
                code,
                &head,
                now_s,
            ) {
                farmerbob_core::limit_signal::Classification::Limited { .. } => {
                    println!(
                        "{task}--{arm}: no run record; classified from the log, which \
                         records exit status {code}"
                    );
                    farmerbob_core::outcome::OutcomeClass::QuotaLimited
                }
                // Ambiguous exists so a caller cannot park on wording alone.
                other => {
                    eprintln!(
                        "fb park: no run record for {task}--{arm}; its log (exit {code}) \
                         classifies as {other:?}, which is not a refusal to park on"
                    );
                    return 2;
                }
            }
        }
    };

    let now_ms = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_millis() as u64,
        Err(_) => {
            eprintln!("fb park: the clock is before the epoch; refusing to compute a park");
            return 1;
        }
    };

    let d = decide(
        arm,
        class,
        &head,
        &core_registry(&reg),
        now_ms,
        default_backoff_secs.saturating_mul(1000),
    );

    match &d {
        Decision::Leave => {
            println!(
                "{task}--{arm}: {class:?} -> leave. This run says nothing about availability."
            );
            0
        }
        // A REFUSAL HAPPENED AND ITS WIDTH IS UNKNOWN. This is neither a park nor a Leave:
        // parking a credential on wording nobody taught us would be a guess, and treating
        // it as Leave would report "says nothing about availability" about a run that was
        // refused. Exit 3 so a caller can tell it from both.
        Decision::UnknownBlast {
            arm: refused,
            evidence,
        } => {
            println!("{task}--{arm}: {class:?} -> REFUSED, width unknown");
            println!("  {refused} was refused and the wording matches no known phrasing,");
            println!("  so how far the refusal reaches cannot be decided here.");
            println!("  evidence: {evidence}");
            println!(
                "  known phrasings: {}",
                farmerbob_core::quota::known_phrasings()
                    .iter()
                    .map(|(p, _)| *p)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            println!("  Add the phrasing to farmerbob_core::quota, or park by hand.");
            3
        }
        Decision::ParkUntil { arms, blast, .. } | Decision::ParkFor { arms, blast, .. } => {
            let until_ms = match &d {
                Decision::ParkUntil { at_ms, .. } => *at_ms,
                Decision::ParkFor { backoff_ms, .. } => now_ms.saturating_add(*backoff_ms),
                // `d` was matched as a park two lines above, so neither of these can reach
                // here. Returning the current instant rather than panicking: an
                // unreachable! in a command that EDITS THE REGISTRY is a crash where a
                // no-op park would do.
                Decision::Leave | Decision::UnknownBlast { .. } => now_ms,
            };
            let when = format_instant(until_ms);
            println!("{task}--{arm}: {class:?}");
            println!("  blast: {} ({} arm(s))", blast_word(*blast), arms.len());
            println!("  until: {when}");
            match grounds(&d) {
                Some("") => println!("  grounds: (the provider recorded none)"),
                Some(g) => println!("  grounds: {}", g.lines().next().unwrap_or(g)),
                None => {}
            }
            for a in arms {
                println!("    {a}");
            }
            if !apply {
                println!();
                println!(
                    "DRY RUN. sources.toml was not changed. Re-run with --apply to park these arms."
                );
                return 0;
            }
            match apply_parks(&when, arms) {
                Ok(n) => {
                    println!("parked {n} arm(s) until {when}");
                    0
                }
                Err(e) => {
                    eprintln!("fb park: {e}");
                    1
                }
            }
        }
    }
}

/// Set `parked_until` on each named arm in `sources.toml`, in place.
///
/// Textual, deliberately: the registry carries comments that explain every past
/// park and a serde round-trip would delete all of them. Those comments are the
/// only record of why an arm was benched.
fn apply_parks(when: &str, arms: &[String]) -> Result<usize, String> {
    let path = crate::paths::repo().join("sources.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("reading sources.toml: {e}"))?;
    let mut out = String::with_capacity(text.len() + arms.len() * 64);
    let mut n = 0;
    let mut current: Option<String> = None;
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("[source.") {
            current = rest.strip_suffix(']').map(str::to_string);
        }
        // Drop any existing parked_until for an arm we are about to re-park, so the
        // file never carries two.
        let in_target = current
            .as_deref()
            .is_some_and(|c| arms.iter().any(|a| a == c));
        if in_target && line.trim_start().starts_with("parked_until") {
            continue;
        }
        out.push_str(line);
        out.push('\n');
        if in_target && line.trim().starts_with("[source.") {
            out.push_str(&format!("parked_until = \"{when}\"   # fb park\n"));
            n += 1;
        }
    }
    std::fs::write(&path, out).map_err(|e| format!("writing sources.toml: {e}"))?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A CRITIC writes `critiques/<task>/<arm>.log`, not `<task>--<arm>.log`. Resolving only
    /// the implementer layout meant `fb park <task> <critic>` answered "nothing to classify"
    /// -- so the quota chain could not classify a critic run even when called by hand at the
    /// right moment with the right arguments. agy-opus-46 hit a provider quota as a critic
    /// twice on 2026-09-19 and both parks had to be written by hand.  (bead farmerbob-4uha)
    #[test]
    fn a_critics_log_is_found_as_well_as_an_implementers() {
        let logs = std::path::Path::new("/l");
        let paths = log_paths(logs, "novelty", "agy-opus-46");
        assert!(paths.contains(&logs.join("novelty--agy-opus-46.log")));
        assert!(
            paths.contains(
                &logs
                    .join("critiques")
                    .join("novelty")
                    .join("agy-opus-46.log")
            )
        );
    }

    /// The implementer layout is tried FIRST, so nothing about existing callers changes.
    #[test]
    fn the_implementer_layout_still_wins() {
        let paths = log_paths(std::path::Path::new("/l"), "t", "a");
        assert!(
            paths[0].to_string_lossy().ends_with("t--a.log"),
            "{paths:?}"
        );
    }

    /// A CRITIC WRITES NO RUN RECORD, so `fb park` refused and nothing could park it. The
    /// exit status is in the log all the same -- `launch_in` records it -- and that is
    /// evidence the harness gathered rather than a guess.
    ///
    /// agy-opus-46 and agy-sonnet-46 each burned a critic dispatch on the same account-wide
    /// quota on 2026-09-19, twelve minutes apart, and neither was parked. (farmerbob-4uha)
    #[test]
    fn the_exit_status_is_read_from_a_critics_log() {
        let head = "InstrumentFailed { reason: \"agy-sonnet-46 exited with exit status: 3: \
                    \\n=== stderr ===\\nerror: Individual quota reached.\" }";
        assert_eq!(exit_code_from_log(head), Some(3));
    }

    /// A log with no recorded status yields None, and the caller refuses rather than
    /// inventing one. The exit code is load-bearing: `limit_signal::classify` returns
    /// Ambiguous for refusal-shaped wording seen with code 0, precisely so nothing parks on
    /// wording alone.
    #[test]
    fn a_log_without_a_recorded_status_yields_nothing() {
        assert_eq!(exit_code_from_log("error: Individual quota reached."), None);
        assert_eq!(exit_code_from_log(""), None);
        assert_eq!(exit_code_from_log("exited with exit status: "), None);
    }

    /// Zero is a real recorded status and must come back as such, not as "absent".
    #[test]
    fn a_recorded_zero_is_a_status_not_an_absence() {
        assert_eq!(
            exit_code_from_log("arm exited with exit status: 0 fine"),
            Some(0)
        );
    }
}
