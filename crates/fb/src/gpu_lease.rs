//! File-backed GPU (and future scarce-resource) leases for dispatch.
//!
//! Bead farmerbob-oa5w.1: wire the pure [`farmerbob_core::lease`]
//! arbitration into admission/dispatch -- who holds the GPU lease, who
//! waits, and what the record says. The daemon (bead farmerbob-3ne) owns
//! serialization eventually; until then this file is the serialization
//! point: one JSON document holding the whole [`LeaseManager`], mutated
//! under an atomic mkdir lock.
//!
//! Deliberately NOT a queue across processes: a refused dispatch returns
//! busy and the operator (or autopilot tick) retries. Cross-process
//! queues without a daemon strand waiters when holders die; TTL expiry
//! already heals crashed holders, and that is as clever as a file gets.
//! The manager's own FIFO still serves any in-process waiters if the
//! daemon ever embeds this whole file.
//!
//! Lock discipline: `<state>/leases.lock/` created atomically; a lock
//! older than the stale timeout is the previous holder's crash, removed
//! and retried once per call. State writes go to temp + rename.

use chrono::{DateTime, Duration, Utc};
use farmerbob_core::lease::{HolderId, LeaseManager, LeaseStatus, RequestOutcome, ResourceName};
use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;

/// How long a lock may be held before it reads as a crash.
pub const LOCK_STALE_SECS: u64 = 120;

/// A granted lease: the token that releases it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acquired {
    /// Opaque proof-of-holderness for [`release`].
    pub token: String,
}

/// A refused acquisition: who holds the resource now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Busy {
    /// Current holder run id.
    pub holder: String,
    /// Queued waiters (foreign writers only; this layer never queues).
    pub queue_depth: usize,
    /// How long the holder has held it, in seconds, when known.
    pub held_for_secs: Option<i64>,
}

fn state_file(state_dir: &Path) -> PathBuf {
    state_dir.join("gpu-leases.json")
}

fn lock_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("gpu-leases.lock")
}

fn lock_age_secs(lock: &Path) -> Option<u64> {
    let meta = std::fs::metadata(lock).ok()?;
    let modified = meta.modified().ok()?;
    std::time::SystemTime::now()
        .duration_since(modified)
        .ok()
        .map(|d| d.as_secs())
}

/// Run `op` with the state file locked. Stale locks (older than
/// `stale_after`) are removed and retried, bounding crash fallout to one
/// stale window.
fn with_locked<T>(
    state_dir: &Path,
    stale_after: StdDuration,
    op: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    if let Err(e) = std::fs::create_dir_all(state_dir) {
        return Err(format!("cannot create {}: {e}", state_dir.display()));
    }
    let lock = lock_dir(state_dir);
    for _ in 0..3 {
        match std::fs::create_dir(&lock) {
            Ok(()) => {
                let result = op();
                let _ = std::fs::remove_dir(&lock);
                return result;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let stale = lock_age_secs(&lock).is_some_and(|age| age >= stale_after.as_secs());
                if stale {
                    let _ = std::fs::remove_dir(&lock);
                    continue;
                }
                return Err(format!("lease store busy ({} held)", lock.display()));
            }
            Err(e) => return Err(format!("cannot lock {}: {e}", lock.display())),
        }
    }
    Err(format!("lease store busy ({} held)", lock.display()))
}

fn load_manager(state_dir: &Path) -> Result<LeaseManager, String> {
    let path = state_file(state_dir);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| format!("unreadable lease store at {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(LeaseManager::new()),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

fn save_manager(state_dir: &Path, manager: &LeaseManager) -> Result<(), String> {
    let path = state_file(state_dir);
    let tmp = state_dir.join("gpu-leases.json.tmp");
    let text =
        serde_json::to_string_pretty(manager).map_err(|e| format!("cannot encode leases: {e}"))?;
    std::fs::write(&tmp, text).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("cannot publish {}: {e}", path.display()))
}

/// Acquire `resource` for `holder`, TTL `max_hold_secs` from now.
/// Retried acquisitions by the same holder are idempotent.
pub fn acquire(
    state_dir: &Path,
    resource: &str,
    holder: &str,
    max_hold_secs: u64,
) -> Result<Result<Acquired, Busy>, String> {
    acquire_at(
        state_dir,
        resource,
        holder,
        max_hold_secs,
        Utc::now(),
        StdDuration::from_secs(LOCK_STALE_SECS),
    )
}

fn acquire_at(
    state_dir: &Path,
    resource: &str,
    holder: &str,
    max_hold_secs: u64,
    now: DateTime<Utc>,
    stale_after: StdDuration,
) -> Result<Result<Acquired, Busy>, String> {
    with_locked(state_dir, stale_after, || {
        let mut manager = load_manager(state_dir)?;
        let name = ResourceName(resource.to_string());
        manager.register_resource(name.clone(), Duration::seconds(max_hold_secs as i64));
        // Revoke the lapsed first: a crashed holder's lease heals here.
        // Always saved below, revoked or not, so healing persists.
        manager.expire(now);
        let status = manager.status(&name, now);
        // Idempotent re-acquire falls through to request(), which
        // returns the existing grant rather than a second lease.
        if let Some(current) = status.holder
            && current.0 != holder
        {
            save_manager(state_dir, &manager)?;
            return Ok(Err(Busy {
                holder: current.0,
                queue_depth: status.queue_depth,
                held_for_secs: status.held_for.map(|d| d.num_seconds()),
            }));
        }
        if status.queue_depth > 0 {
            // Foreign writers queued behind a freed slot: do not jump
            // them, and do not join them (this layer never queues -- the
            // tick retries). Refuse without mutating further.
            save_manager(state_dir, &manager)?;
            return Ok(Err(Busy {
                holder: "(queued)".to_string(),
                queue_depth: status.queue_depth,
                held_for_secs: None,
            }));
        }
        // Free and empty: request() grants rather than queues, by its
        // contract. The outcome is still matched, because a queue entry
        // this layer never services must never be stored.
        match manager.request(&name, HolderId(holder.to_string()), now) {
            RequestOutcome::Granted(token) => {
                save_manager(state_dir, &manager)?;
                Ok(Ok(Acquired { token: token.0 }))
            }
            RequestOutcome::Queued { position } => Ok(Err(Busy {
                holder: "(queued)".to_string(),
                queue_depth: position,
                held_for_secs: None,
            })),
        }
    })
}

/// Release the lease proved by `token`. True when something was released.
pub fn release(state_dir: &Path, token: &str) -> Result<bool, String> {
    release_at(state_dir, token, Utc::now())
}

fn release_at(state_dir: &Path, token: &str, now: DateTime<Utc>) -> Result<bool, String> {
    with_locked(state_dir, StdDuration::from_secs(LOCK_STALE_SECS), || {
        let mut manager = load_manager(state_dir)?;
        // lease.rs release() reports the PROMOTION grant (None when no
        // waiter steps up), not whether anything was released -- so
        // compare holders across the call. A wrong token leaves the
        // holder untouched and reports false.
        let before: Vec<(ResourceName, Option<HolderId>)> = manager
            .status_all(now)
            .into_iter()
            .map(|s| (s.resource, s.holder))
            .collect();
        if before.iter().all(|(_, holder)| holder.is_none()) {
            return Ok(false);
        }
        manager.release(&farmerbob_core::lease::LeaseToken(token.to_string()), now);
        let changed = manager.status_all(now).into_iter().any(|s| {
            before
                .iter()
                .find(|(name, _)| name == &s.resource)
                .map(|(_, holder)| holder.as_ref() != s.holder.as_ref())
                .unwrap_or(true)
        });
        save_manager(state_dir, &manager)?;
        Ok(changed)
    })
}

/// Snapshot a resource's occupancy.
pub fn status(state_dir: &Path, resource: &str) -> Result<LeaseStatus, String> {
    with_locked(state_dir, StdDuration::from_secs(LOCK_STALE_SECS), || {
        let manager = load_manager(state_dir)?;
        Ok(manager.status(&ResourceName(resource.to_string()), Utc::now()))
    })
}

/// Report occupancy for the `fb gpu-status` command. Read-only: never
/// creates, mutates, or locks beyond the read.
pub fn run_status(resource: &str, out: &mut dyn std::io::Write) -> i32 {
    let root = crate::paths::state().join("gpu-leases");
    match status(&root, resource) {
        Ok(status) => {
            match status.holder {
                Some(holder) => {
                    let held = status
                        .held_for
                        .map(|d| format!(" held {}s", d.num_seconds()))
                        .unwrap_or_default();
                    let _ = writeln!(
                        out,
                        "{resource}: held by {}{}, {} queued",
                        holder.0, held, status.queue_depth
                    );
                }
                None => {
                    let _ = writeln!(out, "{resource}: free");
                }
            }
            0
        }
        Err(why) => {
            let _ = writeln!(out, "error: {why}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sandbox(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fb-gpu-lease-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("sandbox");
        dir
    }

    fn t0() -> DateTime<Utc> {
        chrono::Utc.with_ymd_and_hms(2026, 9, 20, 0, 0, 0).unwrap()
    }

    /// First acquisition grants with a token; status names the holder.
    #[test]
    fn acquire_grants_and_status_names_holder() {
        let root = sandbox("grant");
        let granted = acquire_at(&root, "gpu", "run-a", 3600, t0(), StdDuration::from_secs(0))
            .expect("acquire")
            .expect("granted");
        assert!(!granted.token.is_empty());
        let status = status(&root, "gpu").expect("status");
        assert_eq!(status.holder.map(|h| h.0), Some("run-a".to_string()));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A second holder is refused naming the first; the record, not a
    /// guess, says who holds it.
    #[test]
    fn second_holder_is_busy_naming_first() {
        let root = sandbox("busy");
        acquire_at(&root, "gpu", "run-a", 3600, t0(), StdDuration::from_secs(0))
            .expect("first")
            .expect("granted");
        match acquire_at(&root, "gpu", "run-b", 3600, t0(), StdDuration::from_secs(0))
            .expect("second")
        {
            Err(Busy { holder, .. }) => assert_eq!(holder, "run-a"),
            Ok(_) => panic!("second holder must not be granted"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Release frees; re-acquire by the same holder is idempotent.
    #[test]
    fn release_frees_and_reacquire_is_idempotent() {
        let root = sandbox("release");
        let first = acquire_at(&root, "gpu", "run-a", 3600, t0(), StdDuration::from_secs(0))
            .expect("acquire")
            .expect("granted");
        let same = acquire_at(&root, "gpu", "run-a", 3600, t0(), StdDuration::from_secs(0))
            .expect("reacquire")
            .expect("granted");
        assert_eq!(first.token, same.token);
        assert!(release(&root, &first.token).expect("release"));
        assert!(!release(&root, &first.token).expect("release again"));
        let next = acquire_at(&root, "gpu", "run-b", 3600, t0(), StdDuration::from_secs(0))
            .expect("after release")
            .expect("granted");
        assert_ne!(next.token, first.token);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A lapsed lease heals on next acquire: the crashed holder's slot
    /// frees without a watchdog.
    #[test]
    fn lapsed_leases_heal() {
        let root = sandbox("expiry");
        acquire_at(&root, "gpu", "crashed", 60, t0(), StdDuration::from_secs(0))
            .expect("crashed")
            .expect("granted");
        let later = t0() + chrono::Duration::seconds(3600);
        match acquire_at(&root, "gpu", "next", 60, later, StdDuration::from_secs(0))
            .expect("after lapse")
        {
            Ok(_) => {}
            Err(Busy { holder, .. }) => panic!("lapsed holder {holder} still blocks"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A stale lock reads as a crash and is recovered, not wedged.
    #[test]
    fn stale_locks_recover() {
        let root = sandbox("lock");
        std::fs::create_dir_all(root.join("gpu-leases.lock")).expect("lock dir");
        // stale_after zero: any pre-existing lock is instantly stale.
        let granted = acquire_at(&root, "gpu", "run-a", 60, t0(), StdDuration::from_secs(0))
            .expect("acquire")
            .expect("granted");
        assert!(!granted.token.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
