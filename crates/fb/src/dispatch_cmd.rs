//! `fb dispatch <arm> <task> <prompt> [crate]` — run one arm on one task, confined.
//!
//! Ported from fb-dispatch.sh. The decisions are pure and tested here; the process
//! orchestration -- worktree, systemd scope, launch -- is not, because it cannot be.

use farmerbob_core::target_decl::{self, Declaration};
use std::fs;
use std::path::{Path, PathBuf};

/// Paths the harness strips from every worktree before an arm starts.
///
/// The arm must not be handed the orchestrator's issue tracker, the other tasks' prompts,
/// or the hidden conformance suites -- `.fb/` is TRACKED, so every worktree shipped the
/// tests an arm was about to be measured against.
pub const STRIPPED: [&str; 10] = [
    "CLAUDE.md",
    "AGENTS.md",
    ".beads",
    ".cursor",
    ".codex",
    ".agents",
    ".fb/conformance",
    ".fb/defects",
    ".fb/prompts",
    ".fb/known-defects",
];

/// Whether the task's precondition holds on the base it will run against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Precondition {
    /// The task can run.
    Holds,
    /// It cannot, and this is why. A `creates` task whose target exists would correctly
    /// no-op; a `modifies` task whose target is missing has nothing to modify. Either way
    /// the arm is not at fault and the run must not be scored as its failure.
    Invalid(String),
    /// The spec declares nothing. Other stages report that; it is not dispatch's refusal.
    NoDeclaration,
}

/// Check every declared deliverable against the base.
pub fn precondition(spec: &str, exists: &impl Fn(&str) -> bool) -> Precondition {
    let Ok(declarations) = target_decl::declared_all(spec) else {
        return Precondition::NoDeclaration;
    };
    for d in &declarations {
        let path = target_decl::path(d);
        match d {
            Declaration::Creates(_) if exists(path) => {
                return Precondition::Invalid(format!(
                    "declares creates:{path} but it already exists on the base"
                ));
            }
            Declaration::Modifies(_) if !exists(path) => {
                return Precondition::Invalid(format!(
                    "declares modifies:{path} but it does not exist on the base"
                ));
            }
            _ => {}
        }
    }
    Precondition::Holds
}

/// Why an `fb:reads` path is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadRefusal {
    /// Absolute: it names something outside the worktree by construction.
    Absolute,
    /// Contains `..`: it escapes the repository.
    Escapes,
    /// A dotfile or dot-directory: that is where the harness keeps what the arm must not
    /// see, and `fb:reads .fb/conformance` would hand an arm its own hidden suite.
    Dotted,
}

/// The extra files a spec asks to be restored, validated.
///
/// A task ABOUT a harness script needs that script present, which the strip above removes.
/// Every path is checked, because this is the one place a spec can name a file to copy in.
pub fn reads_of(spec: &str) -> Vec<Result<String, (String, ReadRefusal)>> {
    let mut out = Vec::new();
    for line in spec.lines() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix("<!-- fb:reads ") else {
            continue;
        };
        let Some(body) = rest.strip_suffix("-->") else {
            continue;
        };
        for path in body.split_whitespace() {
            out.push(validate_read(path));
        }
    }
    out
}

fn validate_read(path: &str) -> Result<String, (String, ReadRefusal)> {
    if path.starts_with('/') {
        return Err((path.to_string(), ReadRefusal::Absolute));
    }
    if path == ".." || path.split('/').any(|c| c == "..") {
        return Err((path.to_string(), ReadRefusal::Escapes));
    }
    if path.starts_with('.') {
        return Err((path.to_string(), ReadRefusal::Dotted));
    }
    Ok(path.to_string())
}

/// The prompt an arm is given when it already wrote this task's spec critique.
///
/// Without this, codex resumed its critique session and re-answered the spec instead of
/// implementing it (bead farmerbob-81tg).
pub fn continuation_prompt(task: &str) -> String {
    format!(
        "Your spec critique of this task is COMPLETE and has been recorded. This turn is the\n\
         IMPLEMENTATION turn: write the code the task below specifies. Do not critique the spec\n\
         again and do not write .fb/speccheck.md -- that file belongs to the previous turn and\n\
         anything you write to it now is discarded.\n\
         Everything you found while critiquing still applies. Where the spec is defective, the\n\
         task below says what to do: implement the closest honest thing and say so in your\n\
         handoff.\n\
         {task}"
    )
}

/// The run's identifiers.
pub fn run_id(task: &str, arm: &str) -> String {
    format!("{task}--{arm}")
}

/// The systemd unit name for a run: a scope name may carry only a restricted alphabet.
pub fn unit_name(run: &str, pid: u32) -> String {
    let safe: String = run
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("fb-{safe}-{pid}")
}

/// Remove everything the harness owns from a worktree.
pub fn strip(wt: &Path) {
    for entry in STRIPPED {
        let p = wt.join(entry);
        if p.is_dir() {
            let _ = fs::remove_dir_all(&p);
        } else {
            let _ = fs::remove_file(&p);
        }
    }
    // The harness's own scripts, and any queue file that rode along.
    if let Ok(entries) = fs::read_dir(wt) {
        for e in entries.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with("fb-") && n.ends_with(".sh") {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    if let Ok(entries) = fs::read_dir(wt.join(".fb")) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().ends_with(".tsv") {
                let _ = fs::remove_file(e.path());
            }
        }
    }
}

/// Seed the credentials an isolated XDG environment would otherwise hide.
///
/// opencode keeps provider keys in `$XDG_DATA_HOME/opencode/auth.json`, and the per-run
/// isolation handed every run a fresh empty directory -- so three arms across two vendors
/// died in under eight seconds each with a bare "Unexpected server error" that named
/// neither auth nor us (bead farmerbob-kpch).
pub fn seed_credentials(state_data: &Path) -> Result<(), String> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Err("HOME is unset".to_string());
    };
    let cred = home.join(".local/share/opencode/auth.json");
    if !cred.is_file() {
        return Ok(());
    }
    let dest = state_data.join("opencode");
    fs::create_dir_all(&dest).map_err(|e| format!("cannot create {}: {e}", dest.display()))?;
    fs::copy(&cred, dest.join("auth.json"))
        .map(|_| ())
        .map_err(|e| format!("cannot seed {}: {e}", cred.display()))
}

/// Whether a kernel task demands exclusive GPU access, from its
/// manifest. Unreadable or unparseable manifests (or no `gpu` in
/// `exclusive`) mean no lease: other stages report malformed manifests,
/// and a task that does not declare exclusivity is not serialized.
pub fn kernel_wants_gpu(task_dir: &Path) -> bool {
    std::fs::read_to_string(task_dir.join("task.toml"))
        .ok()
        .and_then(|text| farmerbob_core::task_contract::TaskManifest::parse(&text).ok())
        .is_some_and(|manifest| manifest.exclusive.iter().any(|e| e == "gpu"))
}

/// A held GPU lease, released back to the file store on drop -- every
/// path out, including panics.
pub struct GpuLeaseGuard {
    state_dir: PathBuf,
    token: String,
    arm: String,
}

impl Drop for GpuLeaseGuard {
    fn drop(&mut self) {
        if self.token.is_empty() {
            return;
        }
        if crate::gpu_lease::release(&self.state_dir, &self.token).unwrap_or(false) {
            println!("{}: released gpu lease", self.arm);
        }
    }
}

/// Acquire the GPU lease for a run, or refuse with the reason to print.
/// A broken lease store warns and proceeds unleashed: failing closed on
/// lease-infra trouble would turn every lease bug into a total GPU
/// outage, which is exactly the arrangement this replaces.
pub fn acquire_gpu_lease(run: &str, runtime_max_s: u64) -> Result<GpuLeaseGuard, String> {
    let state_dir = crate::paths::state().join("gpu-leases");
    let max_hold = runtime_max_s.saturating_add(300);
    match crate::gpu_lease::acquire(&state_dir, "gpu", run, max_hold) {
        Ok(Ok(granted)) => Ok(GpuLeaseGuard {
            state_dir,
            token: granted.token,
            arm: run.to_string(),
        }),
        Ok(Err(busy)) => {
            let held = busy
                .held_for_secs
                .map(|s| format!(" held {s}s"))
                .unwrap_or_default();
            Err(format!(
                "GPU-BUSY held by {}{} ({} queued behind it): deferred, not failed",
                busy.holder, held, busy.queue_depth
            ))
        }
        Err(why) => {
            eprintln!("warning: gpu lease unavailable ({why}); proceeding unleashed");
            Ok(GpuLeaseGuard {
                state_dir,
                token: String::new(),
                arm: run.to_string(),
            })
        }
    }
}
///
/// Paths use `/` separators. Unreadable files are skipped, not failed: the
/// snapshot pins what it could read, and a file the harness could not read
/// before dispatch that appears readable after is the operator's cue, not a
/// silent pass. `kernel_trust::snapshot` filters the writable candidate.
pub fn collect_task_files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(base: &Path, rel: &str, out: &mut Vec<(String, Vec<u8>)>) {
        let dir = if rel.is_empty() {
            base.to_path_buf()
        } else {
            base.join(rel)
        };
        let Ok(entries) = fs::read_dir(&dir) else {
            return;
        };
        let mut names: Vec<String> = entries
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for name in names {
            let path = dir.join(&name);
            let rel_path = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            if path.is_dir() {
                walk(base, &rel_path, out);
            } else if let Ok(bytes) = fs::read(&path) {
                out.push((rel_path, bytes));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, "", &mut out);
    out
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Archive a kernel run's work product (bead farmerbob-x81s.16).
///
/// `candidate.py` lives in the SHARED task dir, so the next run on the
/// task overwrites it; the worktree has no copy either, which is why
/// `fb archive` (worktrees only) never sees kernel work. The artifact
/// store (content hash + pin, GC-proof) is where the bytes survive, under
/// the run id. The trust record already persists beside the run log, so
/// only the candidate needs storing. Returns lines for the operator.
/// A store failure warns, never fails the run: archiving is evidence,
///
/// Task dirs are the shared base, like the repo itself: `fb reap` walks
/// worktrees only and must never touch them, and there is nothing per-run
/// in them to reap. `fb kernel-score` already takes task paths, and the
/// trust record above is the scope gate for kernels.
pub fn archive_kernel_run(task_dir: &Path, run: &str, artifacts: &Path) -> Vec<String> {
    let candidate = task_dir.join(farmerbob_core::kernel_trust::WRITABLE);
    let mut sink = Vec::new();
    let code = crate::artifact_cmd::store(&candidate, "kernel-source", run, artifacts, &mut sink);
    let stored = String::from_utf8_lossy(&sink).into_owned();
    if code == 0 {
        vec![format!("archived candidate -> {}", stored.trim())]
    } else {
        vec![format!(
            "warning: candidate not archived (rc={code}): {}",
            stored.trim()
        )]
    }
}
/// Dispatch one arm on one task.
/// What a dispatch must do to the arm's worktree and session before it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provisioning {
    /// A new run: tear the worktree down, recreate it from the base, wipe the session.
    Fresh,
    /// A follow-up turn: leave the worktree and the session exactly as the arm left them.
    Resume,
    /// Asked to resume a worktree that is not there.
    NoWorktree(String),
}

/// Decide between starting a run and continuing one.
///
/// RESUMING MUST NEVER FALL BACK TO FRESH. A fresh dispatch runs `git worktree remove
/// --force`, `git branch -D` and `remove_dir_all` over the session directory -- it destroys
/// the arm's work and the session that makes resumption possible. That is the correct thing
/// to do when starting, and the exact opposite of what a follow-up turn wants: the whole
/// point is to hand the arm back the code a critic just reviewed.
///
/// So a `--continue` whose worktree has gone is REFUSED by name rather than quietly
/// becoming a first run. Silently starting over would look like the follow-up being
/// addressed while the reviewed code was deleted underneath it.
pub fn provisioning(continue_session: bool, worktree_exists: bool) -> Provisioning {
    match (continue_session, worktree_exists) {
        (false, _) => Provisioning::Fresh,
        (true, true) => Provisioning::Resume,
        (true, false) => Provisioning::NoWorktree(
            "asked to continue, but the arm's worktree is gone -- refusing rather than \
             silently starting a fresh run over the code the follow-up is about"
                .to_string(),
        ),
    }
}

pub fn run(arm: &str, task: &str, prompt_file: &Path, krate: &str) -> i32 {
    run_in(arm, task, prompt_file, krate, false)
}

/// `run`, with the choice between a first run and a follow-up turn made explicit.
pub fn run_in(
    arm: &str,
    task: &str,
    prompt_file: &Path,
    krate: &str,
    continue_session: bool,
) -> i32 {
    let repo = crate::paths::repo();
    let wt_root = crate::paths::worktrees();
    let logs = crate::paths::logs();
    let run = run_id(task, arm);
    let wt = wt_root.join(&run);
    let branch = format!("fb/{task}/{arm}");
    let base = std::env::var("FB_BASE").unwrap_or_else(|_| "HEAD".to_string());
    let _ = fs::create_dir_all(&wt_root);
    let _ = fs::create_dir_all(&logs);

    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(&repo)
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    let wt_s = wt.to_string_lossy().to_string();
    let mode = provisioning(continue_session, wt.is_dir());
    if let Provisioning::NoWorktree(why) = &mode {
        println!("{arm}: {why}");
        return 1;
    }
    let resuming = mode == Provisioning::Resume;
    if !resuming {
        let _ = git(&["worktree", "remove", "--force", &wt_s]);
        let _ = git(&["worktree", "prune"]);
        let _ = git(&["branch", "-D", &branch]);
        if !git(&["worktree", "add", "-q", "-b", &branch, &wt_s, &base]) {
            println!("{arm}: worktree failed");
            return 1;
        }
    }

    let Ok(spec) = fs::read_to_string(prompt_file) else {
        println!("{arm}: cannot read {}", prompt_file.display());
        return 1;
    };
    if !resuming {
        // The precondition is checked against the WORKTREE, which is the base the arm will
        // actually see.
        //
        // It is checked ONLY on a fresh run. An `fb:creates` precondition asserts the file
        // does NOT exist yet, and on a follow-up turn it does -- the arm wrote it. Checking
        // here would refuse every continuation of a creates-task as TASK-INVALID, which is
        // the arm being punished for having done the work.
        let exists = |p: &str| wt.join(p).exists();
        if let Precondition::Invalid(why) = precondition(&spec, &exists) {
            println!("{arm:<24} {:<13} {why}", "TASK-INVALID");
            return 0;
        }
        let _ = fs::write(wt.join(".fb-task.md"), &spec);
        strip(&wt);
    }

    for read in if resuming {
        Vec::new()
    } else {
        reads_of(&spec)
    } {
        match read {
            Ok(path) => {
                let src = repo.join(&path);
                if !src.is_file() {
                    println!("{arm}: fb:reads names {path}, which does not exist");
                    return 1;
                }
                if let Some(parent) = wt.join(&path).parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let _ = fs::copy(&src, wt.join(&path));
                println!("{arm}: restored {path} for a task that is about it");
            }
            Err((path, why)) => {
                println!("{arm}: refusing fb:reads {path} ({why:?})");
                return 1;
            }
        }
    }

    let state = crate::paths::state().join("state").join(&run);
    // The session directory is what makes a continuation possible at all: it holds the
    // launcher's own record of the turn the arm is being asked to resume.
    if !resuming {
        let _ = fs::remove_dir_all(&state);
    }
    for sub in ["data", "state", "cache"] {
        let _ = fs::create_dir_all(state.join(sub));
    }
    if let Err(e) = seed_credentials(&state.join("data")) {
        eprintln!("dispatch: {e}");
        return 1;
    }

    // An arm that already wrote this task's spec critique is CONTINUED, and told the
    // critique turn is over.
    let critiqued = logs
        .join("speccheck")
        .join(task)
        .join(format!("{arm}.findings.md"))
        .is_file();
    let prompt = if resuming {
        println!("{arm}: continuing its session on this task -- worktree and session kept");
        spec.clone()
    } else if critiqued {
        println!("{arm}: continuing its spec-critique session");
        continuation_prompt(&spec)
    } else {
        spec.clone()
    };

    // The KernelBench trust boundary (bead farmerbob-x81s.1). When the
    // operator points a run at a task directory, its pinned files are
    // hashed BEFORE the arm starts (every file except candidate.py), the
    // directory rides into the confinement read-only via the same variable
    // (see `isolated_cmd::run`, which binds the task ro and the candidate
    // back writable LAST), and the hashes are re-checked after. systemd-run
    // --scope execs its child directly, so env set here reaches `fb
    // isolated` inside the scope.
    let kernel_task: Option<PathBuf> = std::env::var_os("FB_KERNEL_TASK_DIR")
        .map(PathBuf::from)
        .filter(|d| d.is_dir());
    // Each turn guards itself: the snapshot is taken in memory before
    // launch (and written to the pinned record for audit), then re-checked
    // after. A follow-up turn re-pins what the previous turn left -- which
    // that turn already verified -- so tampering is attributed to the turn
    // that moved the file.
    let before_snap: Vec<farmerbob_core::kernel_trust::Pinned> = match kernel_task.as_deref() {
        Some(dir) => {
            let snap = farmerbob_core::kernel_trust::snapshot(&collect_task_files(dir));
            let mut out = String::from("[");
            for (i, p) in snap.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&format!(
                    "{{\"file\":\"{}\",\"hash\":\"{}\"}}",
                    json_escape(&p.file),
                    p.hash
                ));
            }
            out.push(']');
            let _ = fs::write(logs.join(format!("{run}.pinned.json")), &out);
            println!("{arm}: pinned {} trust snapshot", dir.display());
            snap
        }
        None => Vec::new(),
    };

    let mut env: Vec<(&str, PathBuf)> = vec![
        ("XDG_DATA_HOME", state.join("data")),
        ("XDG_STATE_HOME", state.join("state")),
        ("XDG_CACHE_HOME", state.join("cache")),
    ];
    if let Some(ref dir) = kernel_task {
        env.push(("FB_KERNEL_TASK_DIR", dir.clone()));
    }
    let started = std::time::Instant::now();
    let unit = unit_name(&run, std::process::id());
    let runtime_max_s: u64 = std::env::var("FB_RUN_TIMEOUT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2700);
    // GPU lease (bead farmerbob-oa5w.1): a kernel task declaring
    // exclusive GPU access serializes on the file-backed lease before
    // launching. A busy GPU refuses the run like a bad premise (no
    // launch, no session, rc 0 -- the tick retries), never as an arm
    // failure. The guard releases on every path out, including panics;
    // a crashed dispatch heals by TTL expiry on the next acquire.
    let _gpu_lease = match kernel_task.as_deref() {
        Some(dir) if kernel_wants_gpu(dir) => match acquire_gpu_lease(&run, runtime_max_s) {
            Ok(guard) => Some(guard),
            Err(refused) => {
                println!("{arm}: {refused}");
                return 0;
            }
        },
        _ => None,
    };
    // Host memory peak from the run's OWN scope (bead farmerbob-05p):
    // the sampler reads <scope>/memory.peak on an interval and keeps the
    // max, stopping when the run ends. No pid is ever discovered, so no
    // foreign command line can pollute the reading. Scopes vanish with
    // their last process, which is why this samples during the run: after
    // is too late.
    let mem_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mem_unit = unit.clone();
    let mem_sampler = std::thread::spawn({
        let stop = std::sync::Arc::clone(&mem_stop);
        move || {
            let interval_ms: u64 = std::env::var("FB_MEM_SAMPLE_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(crate::memsample::SAMPLE_INTERVAL_MS);
            crate::memsample::sample_peak(
                Path::new("/sys/fs/cgroup"),
                &mem_unit,
                std::time::Duration::from_millis(interval_ms),
                &stop,
            )
        }
    });
    let said = crate::launch::launch_in(
        arm,
        &prompt,
        &wt,
        // Either kind of continuation asks the launcher to resume rather than start.
        resuming || critiqued,
        &env,
        Some(crate::launch::Scope {
            unit: &unit,
            memory_max: std::env::var("FB_MEM_MAX").unwrap_or_else(|_| "3G".to_string()),
            runtime_max_s,
        }),
    );
    let seconds = started.elapsed().as_secs();
    let log = logs.join(format!("{run}.log"));
    let (rc, text) = match &said {
        farmerbob_core::measurement::Measurement::Observed(t) => (0, t.clone()),
        farmerbob_core::measurement::Measurement::Missing(reason) => (1, format!("{reason:?}")),
    };
    let _ = fs::write(&log, &text);
    // Stop the memory sampler and take its max. Absent when the scope
    // never appeared (or cgroup v1): the run JSON then carries no memory
    // field at all, which renders as unmeasured -- never as zero.
    mem_stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let mem_peak_mb = mem_sampler
        .join()
        .ok()
        .flatten()
        .map(|bytes| bytes / 1024 / 1024);
    if let Some(mb) = mem_peak_mb {
        println!("{arm}: host peak {mb} MB (own scope)");
    }
    // Re-hash the pinned files AFTER the run. A movement voids the run: it
    // names the file and both hashes on stdout and in the trust record, and
    // `fb kernel-score --tampered` (fed from this record) refuses it any
    // score. Tampered is not low-ranked; it is not ranked at all.
    if let Some(ref dir) = kernel_task {
        let after = farmerbob_core::kernel_trust::snapshot(&collect_task_files(dir));
        let tampers = farmerbob_core::kernel_trust::verify(&before_snap, &after);
        let mut trust = String::from("{\"tampered\":[");
        for (i, t) in tampers.iter().enumerate() {
            if i > 0 {
                trust.push(',');
            }
            trust.push_str(&format!(
                "{{\"file\":\"{}\",\"before\":\"{}\",\"after\":\"{}\"}}",
                json_escape(&t.file),
                t.before,
                t.after
            ));
        }
        trust.push_str("]}");
        let _ = fs::write(logs.join(format!("{run}.trust.json")), &trust);
        for t in &tampers {
            println!("{arm}: {}", farmerbob_core::kernel_trust::void_line(t));
        }
        if tampers.is_empty() {
            println!(
                "{arm}: trust clean ({} pinned files re-hashed)",
                after.len()
            );
        }
        for line in archive_kernel_run(dir, &run, &crate::paths::artifacts()) {
            println!("{arm}: {line}");
        }
    }
    let _ = fs::write(
        logs.join(format!("{run}.json")),
        match mem_peak_mb {
            // Provenance rides with the value: readers trust scoped
            // peaks and treat unsourced values as the pgrep era (bead
            // farmerbob-05p).
            Some(mb) => format!(
                r#"{{"source":"{arm}","bead":"{task}","crate":"{krate}","rc":{rc},"duration_s":{seconds},"worktree":"{}","branch":"{branch}","confined":"yes","mem_peak_mb":{mb},"mem_source":"scope"}}"#,
                wt.display()
            ),
            None => format!(
                r#"{{"source":"{arm}","bead":"{task}","crate":"{krate}","rc":{rc},"duration_s":{seconds},"worktree":"{}","branch":"{branch}","confined":"yes"}}"#,
                wt.display()
            ),
        },
    );
    println!("{arm:<24} rc={rc} {seconds}s -> {}", log.display());
    rc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `creates` task whose target already exists cannot do anything but no-op, and the
    /// arm is not at fault. Pinned against the same declaration on a base where it is
    /// absent: one bit different, opposite answers.
    #[test]
    fn a_creates_task_is_invalid_when_the_target_exists() {
        let spec = "<!-- fb:creates crates/a/b.rs -->\n";
        assert!(matches!(
            precondition(spec, &|_| true),
            Precondition::Invalid(_)
        ));
        assert_eq!(precondition(spec, &|_| false), Precondition::Holds);
    }

    /// And a `modifies` task inverts it. The shell checked only `creates`, so a
    /// modifies-task against a deleted file dispatched and every arm correctly no-opped.
    #[test]
    fn a_modifies_task_is_invalid_when_the_target_is_missing() {
        let spec = "<!-- fb:modifies crates/a/b.rs -->\n";
        assert!(matches!(
            precondition(spec, &|_| false),
            Precondition::Invalid(_)
        ));
        assert_eq!(precondition(spec, &|_| true), Precondition::Holds);
    }

    /// EVERY declared file is checked, not just the first. A grouped task whose second
    /// deliverable is missing is as unrunnable as one whose first is.
    #[test]
    fn every_declared_file_is_checked() {
        let spec = "<!-- fb:modifies a.rs -->\n<!-- fb:modifies b.rs -->\n";
        let only_a = |p: &str| p == "a.rs";
        match precondition(spec, &only_a) {
            Precondition::Invalid(why) => assert!(why.contains("b.rs"), "{why}"),
            other => panic!("expected Invalid about b.rs, got {other:?}"),
        }
    }

    /// No declaration is not dispatch's refusal -- other stages report it.
    #[test]
    fn no_declaration_is_not_this_stages_refusal() {
        assert_eq!(
            precondition("# nothing\n", &|_| false),
            Precondition::NoDeclaration
        );
    }

    /// `fb:reads` is the one place a spec names a file to copy INTO the worktree, so every
    /// escape is refused: absolute paths, `..`, and dotfiles -- the last because `.fb/` is
    /// where the hidden conformance suites live, and `fb:reads .fb/conformance` would hand
    /// an arm the tests it is about to be measured against.
    #[test]
    fn fb_reads_refuses_every_escape() {
        let spec = "<!-- fb:reads /etc/passwd -->\n<!-- fb:reads ../x -->\n\
                    <!-- fb:reads .fb/conformance -->\n<!-- fb:reads fb-isolated -->\n";
        let got = reads_of(spec);
        assert_eq!(got.len(), 4);
        assert_eq!(got[0], Err(("/etc/passwd".into(), ReadRefusal::Absolute)));
        assert_eq!(got[1], Err(("../x".into(), ReadRefusal::Escapes)));
        assert_eq!(got[2], Err((".fb/conformance".into(), ReadRefusal::Dotted)));
        assert_eq!(got[3], Ok("fb-isolated".to_string()));
    }

    /// A `..` in the MIDDLE escapes too, not only at the start.
    #[test]
    fn a_dotdot_anywhere_escapes() {
        assert_eq!(
            validate_read("crates/../../etc/x"),
            Err(("crates/../../etc/x".into(), ReadRefusal::Escapes))
        );
    }

    /// A unit name carries only what systemd accepts, and includes the pid so two runs of
    /// one task cannot collide on a scope.
    #[test]
    fn a_unit_name_is_sanitised_and_unique() {
        let u = unit_name("my.task--arm/x", 42);
        assert!(!u.contains('.') && !u.contains('/'), "{u}");
        assert!(u.ends_with("-42"), "{u}");
        assert_ne!(unit_name("t--a", 1), unit_name("t--a", 2));
    }

    /// The continuation prompt tells the arm its critique turn is over. Without it codex
    /// resumed the critique session and re-answered the spec instead of implementing
    /// (bead farmerbob-81tg).
    #[test]
    fn the_continuation_prompt_ends_the_critique_turn() {
        let p = continuation_prompt("TASK BODY");
        assert!(p.contains("IMPLEMENTATION turn"), "{p}");
        assert!(p.contains("Do not critique the spec"), "{p}");
        assert!(p.ends_with("TASK BODY"), "the task is last");
    }

    /// RESUMING MUST NEVER FALL BACK TO FRESH. A fresh dispatch runs `git worktree remove
    /// --force`, `git branch -D` and wipes the session directory. That is right when
    /// starting and is the exact opposite of a follow-up turn, whose whole point is to hand
    /// the arm back the code a critic just reviewed.
    #[test]
    fn continuing_a_missing_worktree_is_refused_not_restarted() {
        assert!(matches!(
            provisioning(true, false),
            Provisioning::NoWorktree(_)
        ));
        assert_eq!(provisioning(true, true), Provisioning::Resume);
    }

    /// A first run provisions whether or not anything is there: that is what makes it a
    /// first run, and a leftover worktree from a previous wave must not be inherited.
    #[test]
    fn a_first_run_provisions_either_way() {
        assert_eq!(provisioning(false, false), Provisioning::Fresh);
        assert_eq!(provisioning(false, true), Provisioning::Fresh);
    }

    /// The refusal SAYS why. "worktree failed" would read as a git problem rather than as
    /// the harness declining to destroy the work the follow-up is about.
    #[test]
    fn the_refusal_names_what_it_is_protecting() {
        let Provisioning::NoWorktree(why) = provisioning(true, false) else {
            panic!("expected a refusal");
        };
        assert!(why.contains("refusing"), "{why}");
        assert!(why.contains("fresh run"), "{why}");
    }

    /// The trust collector walks the whole task tree (including `ref/`) and
    /// the snapshot pins everything but the candidate (bead
    /// farmerbob-x81s.1).
    #[test]
    fn the_trust_collector_pins_everything_but_the_candidate() {
        let base = std::env::temp_dir().join(format!("fb-trust-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("ref")).unwrap();
        std::fs::write(base.join("candidate.py"), "agent").unwrap();
        std::fs::write(base.join("verify.sh"), "scorer").unwrap();
        std::fs::write(base.join("ref/problem.py"), "ref").unwrap();
        let files = collect_task_files(&base);
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"candidate.py"), "{names:?}");
        assert!(names.contains(&"verify.sh"), "{names:?}");
        assert!(names.contains(&"ref/problem.py"), "{names:?}");
        let snap = farmerbob_core::kernel_trust::snapshot(&files);
        assert!(!snap.iter().any(|p| p.file == "candidate.py"));
        assert!(snap.iter().any(|p| p.file == "verify.sh"));
        // A tampered scorer between two collections is caught end to end.
        std::fs::write(base.join("verify.sh"), "{\"correct\":true}").unwrap();
        let after = farmerbob_core::kernel_trust::snapshot(&collect_task_files(&base));
        let tampers = farmerbob_core::kernel_trust::verify(&snap, &after);
        assert_eq!(tampers.len(), 1);
        assert_eq!(tampers[0].file, "verify.sh");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Archiving stores the candidate under the run id: the bytes survive
    /// the shared task dir being overwritten by the next run (bead
    /// farmerbob-x81s.16). A missing candidate warns instead of failing.
    #[test]
    fn archiving_pins_the_candidate_to_the_run() {
        let base = std::env::temp_dir().join(format!("fb-archive-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let task = base.join("task");
        let store = base.join("store");
        std::fs::create_dir_all(&task).unwrap();
        std::fs::write(task.join("candidate.py"), "agent code").unwrap();
        let lines = archive_kernel_run(&task, "test-run", &store);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("archived candidate -> "), "{lines:?}");
        assert!(lines[0].contains("\"pin\":\"test-run\""), "{lines:?}");
        // The pin record names the run: evidence survives GC and reaping.
        let pin = std::fs::read_to_string(store.join("pins").join("test-run")).expect("pin");
        assert!(pin.contains("kernel_source"), "{pin}");
        // Idempotent: a second archive deduplicates, same pin.
        let again = archive_kernel_run(&task, "test-run", &store);
        assert!(again[0].contains("\"deduplicated\":true"), "{again:?}");
        // Nothing to store warns rather than failing the run.
        let missing = archive_kernel_run(&base.join("empty"), "test-run", &store);
        assert!(missing[0].starts_with("warning: "), "{missing:?}");
        let _ = std::fs::remove_dir_all(&base);
    }
}
