//! `fb admit <matrix.tsv>` — run a dispatch matrix with admission control.
//!
//! Ported from fb-admit.sh. MemoryMax is a per-run LIMIT, not a reservation; systemd will
//! happily accept scopes whose limits exceed the machine, so the SUM is what must be
//! bounded (bead farmerbob-89j).

use farmerbob_core::measurement::Measurement;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// One row of the matrix: a task, its crate, and the arms to run it on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub task: String,
    pub krate: String,
    pub arms: Vec<String>,
}

/// One admitted run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub arm: String,
    pub task: String,
    pub krate: String,
}

/// Why a matrix cannot be run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// A name absent from the registry is a TYPO, and a typo means the matrix does not run
    /// what its author believed. Worth stopping the wave for; a deliberately disabled arm
    /// is not (bead farmerbob-vgn).
    UnregisteredArm(String),
    /// Every arm named is ineligible.
    NothingEligible,
}

/// Parse a matrix file. Blank lines are skipped; a row needs a task and at least one arm.
pub fn parse_matrix(text: &str) -> Vec<Row> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            let mut f = l.split('\t');
            let task = f.next()?.trim().to_string();
            if task.is_empty() {
                return None;
            }
            let krate = f.next().unwrap_or("farmerbob-core").trim().to_string();
            let arms: Vec<String> = f
                .next()
                .unwrap_or("")
                .split(',')
                .map(str::trim)
                .filter(|a| !a.is_empty())
                .map(str::to_string)
                .collect();
            Some(Row { task, krate, arms })
        })
        .collect()
}

/// Which runs to admit, in matrix order.
///
/// `eligible` answers whether an arm may be dispatched; `registered` whether the registry
/// has heard of it at all. The two are different questions and conflating them is how a
/// typo became a silent no-op.
pub fn admit(
    rows: &[Row],
    eligible: &impl Fn(&str) -> bool,
    registered: &impl Fn(&str) -> bool,
) -> Result<Vec<Run>, Refusal> {
    let mut out: Vec<Run> = Vec::new();
    for row in rows {
        for arm in &row.arms {
            if eligible(arm) {
                out.push(Run {
                    arm: arm.clone(),
                    task: row.task.clone(),
                    krate: row.krate.clone(),
                });
            } else if !registered(arm) {
                return Err(Refusal::UnregisteredArm(arm.clone()));
            }
        }
    }
    if out.is_empty() {
        return Err(Refusal::NothingEligible);
    }
    Ok(out)
}

/// Whether one more run of `provider` may start.
///
/// Agents are network-bound, so the machine is not the binding constraint -- the PROVIDER
/// is. Poolside rate-limited a run when free-tier calls were bursted (bead farmerbob-4ix).
pub fn provider_has_room(in_flight: &BTreeMap<String, usize>, provider: &str, cap: usize) -> bool {
    in_flight.get(provider).copied().unwrap_or(0) < cap
}

/// How many runs fit, charging each the HARD cap.
///
/// Zero is an answer. The dispatcher waits for the next tick rather than being told a
/// comfortable number.
pub fn usable_slots(available_mb: u64, headroom_mb: u64, per_run_mb: u64) -> usize {
    if per_run_mb == 0 {
        return 0;
    }
    (available_mb.saturating_sub(headroom_mb) / per_run_mb) as usize
}

/// Available memory in MB, as the kernel reports it.
fn available_mb() -> Measurement<u64> {
    let Ok(text) = fs::read_to_string("/proc/meminfo") else {
        return Measurement::instrument_failed("cannot read /proc/meminfo");
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("MemAvailable:") {
            if let Some(kb) = rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<u64>().ok())
            {
                return Measurement::observed(kb / 1024);
            }
        }
    }
    Measurement::instrument_failed("/proc/meminfo has no MemAvailable")
}

/// The quota bucket that actually rate-limits an arm.
pub fn provider_of(registry: &crate::sources::Registry, arm: &str) -> String {
    registry
        .sources
        .get(arm)
        .map(|s| s.bucket().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Run the matrix.
pub fn run(matrix: &Path) -> i32 {
    let Ok(text) = fs::read_to_string(matrix) else {
        eprintln!("cannot read {}", matrix.display());
        return 1;
    };
    let repo = crate::paths::repo();
    let Ok(registry) = crate::sources::Registry::load(&repo.join("sources.toml")) else {
        eprintln!("cannot read the registry");
        return 1;
    };
    let hard_mb: u64 = std::env::var("FB_MEM_GB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(2)
        * 1024;
    let headroom_mb: u64 = std::env::var("FB_HEADROOM_GB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(3)
        * 1024;
    let cap: usize = std::env::var("FB_PROVIDER_CAP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);

    let avail = match available_mb() {
        Measurement::Observed(mb) => mb,
        Measurement::Missing(reason) => {
            // Refuse rather than fall back to arithmetic: a second copy of this decision is
            // how it stayed wrong for so long.
            eprintln!("admission: cannot size the machine ({reason:?}); refusing to guess");
            return 1;
        }
    };
    // Charge each admission the HARD cap so committed + requested <= available - headroom
    // bounds the SUM. This was two lines of awk against the MEASURED p95 while every run was
    // handed the hard cap as its MemoryMax, and it admitted a run onto a machine measured as
    // unable to hold it (bead farmerbob-89j).
    let slots = usable_slots(avail, headroom_mb, hard_mb);
    println!(
        "admission: {}M/slot (hard cap), {}M headroom, {}M avail -> {slots} slots, max {cap} per upstream vendor",
        hard_mb, headroom_mb, avail
    );
    if slots == 0 {
        eprintln!("admission: 0 slots -- nothing dispatched");
        return 0;
    }

    let rows = parse_matrix(&text);
    let eligible = |a: &str| registry.eligible(a).is_ok();
    let registered = |a: &str| registry.sources.contains_key(a);
    let queue = match admit(&rows, &eligible, &registered) {
        Ok(q) => q,
        Err(Refusal::UnregisteredArm(a)) => {
            println!("abort: {a} is not a registered arm");
            return 2;
        }
        Err(Refusal::NothingEligible) => {
            println!("no eligible runs");
            return 1;
        }
    };
    println!("queued {} runs", queue.len());
    // Dispatch itself is still fb-dispatch.sh until that port lands; the admission half --
    // slots, headroom and the per-provider cap -- is this command.
    let dispatch = repo.join("fb-dispatch.sh");
    let mut children: Vec<(String, std::process::Child)> = Vec::new();
    for run in &queue {
        let provider = provider_of(&registry, &run.arm);
        // Wait for a slot AND for the provider to have room. The machine is not the binding
        // constraint here; the provider is.
        loop {
            // A finished child releases its provider slot.
            children.retain_mut(|(_, c)| !matches!(c.try_wait(), Ok(Some(_))));
            let mut in_flight: BTreeMap<String, usize> = BTreeMap::new();
            for (p, _) in &children {
                *in_flight.entry(p.clone()).or_insert(0) += 1;
            }
            if children.len() < slots && provider_has_room(&in_flight, &provider, cap) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
        println!(
            "dispatch {}/{}  (provider {provider}, running {}/{slots})",
            run.task,
            run.arm,
            children.len()
        );
        let spec = format!(".fb/prompts/{}.md", run.task);
        match std::process::Command::new(&dispatch)
            .args([&run.arm, &run.task, &spec, &run.krate])
            .current_dir(&repo)
            .env("FB_MEM_MAX", format!("{hard_mb}M"))
            .spawn()
        {
            Ok(child) => children.push((provider, child)),
            Err(e) => eprintln!("cannot dispatch {}/{}: {e}", run.task, run.arm),
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    for (_, mut c) in children {
        let _ = c.wait();
    }
    println!("all runs complete");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(spec: &str) -> Vec<Row> {
        parse_matrix(spec)
    }

    /// A matrix row is task, crate, comma-separated arms.
    #[test]
    fn a_matrix_row_parses_into_its_three_fields() {
        let r = rows("t\tfb\ta,b\n");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].task, "t");
        assert_eq!(r[0].krate, "fb");
        assert_eq!(r[0].arms, vec!["a".to_string(), "b".to_string()]);
    }

    /// Blank lines are skipped rather than becoming a row with an empty task, which the
    /// shell's `[ -z "$task" ] && continue` did by hand on every read.
    #[test]
    fn blank_lines_are_not_rows() {
        assert!(rows("\n\n  \n").is_empty());
        assert_eq!(rows("t\tfb\ta\n\n").len(), 1);
    }

    /// An UNREGISTERED arm aborts the wave: a typo means the matrix does not run what its
    /// author believed. A registered but ineligible arm is skipped quietly -- disabling an
    /// arm is a deliberate act and must not stop everyone else's work.
    #[test]
    fn a_typo_aborts_and_a_disabled_arm_does_not() {
        let r = rows("t\tfb\ttypo\n");
        let known = |a: &str| a != "typo";
        assert_eq!(
            admit(&r, &|_| false, &known),
            Err(Refusal::UnregisteredArm("typo".into()))
        );
        let r = rows("t\tfb\tdisabled,ok\n");
        let admitted = admit(&r, &|a: &str| a == "ok", &|_| true).expect("ok is eligible");
        assert_eq!(admitted.len(), 1);
        assert_eq!(admitted[0].arm, "ok");
    }

    /// Every arm ineligible is NothingEligible, not an empty success. A wave that admitted
    /// nothing and returned 0 would look like a wave that ran.
    #[test]
    fn nothing_eligible_is_a_refusal_not_an_empty_run() {
        let r = rows("t\tfb\ta,b\n");
        assert_eq!(
            admit(&r, &|_| false, &|_| true),
            Err(Refusal::NothingEligible)
        );
    }

    /// Zero slots is an ANSWER, not a floor to round up from. A machine with less free
    /// memory than the headroom holds nothing, and admitting one run anyway is how a
    /// measured-unable machine got a run (bead farmerbob-89j).
    #[test]
    fn a_machine_too_small_has_zero_slots_not_one() {
        assert_eq!(usable_slots(4096, 3072, 2048), 0);
        assert_eq!(usable_slots(2048, 3072, 2048), 0, "less free than headroom");
        assert_eq!(usable_slots(7168, 3072, 2048), 2);
    }

    /// A zero per-run budget would divide by zero; it admits nothing instead.
    #[test]
    fn a_zero_budget_admits_nothing() {
        assert_eq!(usable_slots(100_000, 0, 0), 0);
    }

    /// The provider cap is per BUCKET and counts what is in flight. At the cap there is no
    /// room; one below there is. Pinned at the boundary, because the binding constraint is
    /// the provider rather than the machine.
    #[test]
    fn the_provider_cap_binds_at_the_cap() {
        let mut f = BTreeMap::new();
        f.insert("openrouter".to_string(), 3usize);
        assert!(!provider_has_room(&f, "openrouter", 3));
        f.insert("openrouter".to_string(), 2usize);
        assert!(provider_has_room(&f, "openrouter", 3));
        assert!(
            provider_has_room(&f, "never-seen", 3),
            "absent means zero in flight"
        );
    }
}
