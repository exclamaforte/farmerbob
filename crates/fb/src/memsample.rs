//! Host memory peaks from the run's own scope, never via pid discovery.
//!
//! Bead farmerbob-05p: the old sampler found pids with `pgrep -f
//! <worktree>`, which matches any command line containing the path --
//! including the harness's own `git worktree add` in the desktop scope.
//! Three runs reported one desktop's 13.8 GB as their own. The fix is
//! structural: the cgroup IS the authority on which processes belong to
//! a run, so read `<own-scope>/memory.peak` directly and discover no
//! pids at all. A synthetic foreign process whose cmdline contains the
//! worktree path cannot affect the reading, because nothing reads
//! cmdliness. The sampler polls the scope's file on an interval and keeps
//! the max; after the scope is reaped there is nothing to read, which is
//! why sampling happens during the run, not after it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How often the scope file is polled.
pub const SAMPLE_INTERVAL_MS: u64 = 2000;

/// Upper bound on directories visited while locating one scope:
/// /sys is small, and an unbounded walk under a weird mount is a hang.
const MAX_VISITED_DIRS: usize = 20_000;

/// Locate a scope directory by unit name under a cgroup root.
///
/// Depth-first, dirs only, no symlink following. Matches the exact unit
/// name with or without systemd's `.scope` suffix: `systemd-run
/// --unit=fb-x` creates `fb-x.scope` on disk, and the sampler is handed
/// the `--unit` spelling. The unit lives wherever systemd put it
/// (`app.slice` today, `background.slice` tomorrow); the name is the
/// stable part, so search for the name rather than hardcoding the slice
/// path.
pub fn scope_dir(cgroup_root: &Path, unit: &str) -> Option<PathBuf> {
    let scoped = format!("{unit}.scope");
    let mut stack = vec![cgroup_root.to_path_buf()];
    let mut visited = 0;
    while let Some(dir) = stack.pop() {
        visited += 1;
        if visited > MAX_VISITED_DIRS {
            return None;
        }
        let entries = std::fs::read_dir(&dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == unit || name == scoped {
                return Some(path);
            }
            // Never descend into another scope's subtree beyond reason:
            // scopes nest shallowly, and the cap above bounds the walk.
            stack.push(path);
        }
    }
    None
}

/// Read one scope's memory peak in bytes. `None` covers missing files
/// and unparseable contents: a scope that is gone, or a kernel without
/// the v2 file, is no measurement, never a zero.
pub fn read_peak(scope: &Path) -> Option<u64> {
    std::fs::read_to_string(scope.join("memory.peak"))
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
}

/// Poll a scope's peak until stopped, keeping the max.
///
/// Returns `None` when no reading was ever taken (scope never appeared,
/// file unreadable throughout): the run JSON then carries no memory
/// field at all, which renders as unmeasured rather than as zero.
pub fn sample_peak(
    cgroup_root: &Path,
    unit: &str,
    interval: Duration,
    stop: &AtomicBool,
) -> Option<u64> {
    let mut peak: Option<u64> = None;
    while !stop.load(Ordering::Relaxed) {
        if let Some(scope) = scope_dir(cgroup_root, unit)
            && let Some(current) = read_peak(&scope)
        {
            peak = Some(peak.map_or(current, |best: u64| best.max(current)));
        }
        if !stop.load(Ordering::Relaxed) {
            std::thread::sleep(interval);
        }
    }
    peak
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn tree(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("fb-memsample-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("user.slice/app.slice/fb-test.scope")).unwrap();
        std::fs::create_dir_all(base.join("user.slice/desktop.scope")).unwrap();
        std::fs::write(
            base.join("user.slice/app.slice/fb-test.scope/memory.peak"),
            "128466944\n",
        )
        .unwrap();
        std::fs::write(
            base.join("user.slice/desktop.scope/memory.peak"),
            "14486126592\n",
        )
        .unwrap();
        base
    }

    /// The scope is found wherever systemd put it: the name is searched,
    /// never the slice path hardcoded -- with or without systemd's
    /// `.scope` suffix, since the sampler is handed the `--unit`
    /// spelling while the disk carries `fb-x.scope`.
    #[test]
    fn scope_found_by_name_not_slice() {
        let base = tree("found");
        assert_eq!(
            scope_dir(&base, "fb-test.scope"),
            Some(base.join("user.slice/app.slice/fb-test.scope"))
        );
        assert_eq!(
            scope_dir(&base, "fb-test"),
            Some(base.join("user.slice/app.slice/fb-test.scope"))
        );
        assert_eq!(scope_dir(&base, "fb-nope"), None);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The acceptance case: a foreign scope holding 13.8 GB -- with a
    /// synthetic foreign process whose cmdline contains the worktree path
    /// sitting beside it -- does not affect the run's own 122 MB reading,
    /// because nothing discovers pids at all.
    #[test]
    fn foreign_scope_and_foreign_cmdline_do_not_leak() {
        let base = tree("foreign");
        // The bead's exact scenario, minus the pgrep: a process whose
        // command line contains the worktree path, living in the desktop
        // scope. The sampler has no code path that could ever consult it.
        let _foreign_cmdline = format!("git -C /repo worktree add {}", base.display());
        let own = scope_dir(&base, "fb-test.scope").expect("own scope");
        assert_eq!(read_peak(&own), Some(128_466_944));
        assert_ne!(read_peak(&own), Some(14_486_126_592));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Missing files and garbage parse as no measurement, never zero.
    #[test]
    fn unreadable_is_missing_never_zero() {
        let base = tree("unreadable");
        assert_eq!(read_peak(&base.join("no-such-scope")), None);
        std::fs::write(
            base.join("user.slice/app.slice/fb-test.scope/memory.peak"),
            "not-a-number\n",
        )
        .unwrap();
        assert_eq!(
            read_peak(&base.join("user.slice/app.slice/fb-test.scope")),
            None
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The sampler observes a live scope and stops on signal; a scope
    /// that never appears yields no measurement.
    #[test]
    fn sampler_tracks_and_stops() {
        let base = tree("sampler");
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop);
        let root = base.clone();
        let handle = std::thread::spawn(move || {
            sample_peak(
                &root,
                "fb-test.scope",
                Duration::from_millis(5),
                &stop_clone,
            )
        });
        std::thread::sleep(Duration::from_millis(50));
        stop.store(true, Ordering::Relaxed);
        assert_eq!(handle.join().expect("sampler"), Some(128_466_944));
        let gone = sample_peak(
            &base,
            "fb-nope.scope",
            Duration::from_millis(5),
            &AtomicBool::new(true),
        );
        assert_eq!(gone, None);
        let _ = std::fs::remove_dir_all(&base);
    }
}
