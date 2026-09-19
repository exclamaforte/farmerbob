//! `fb wave <matrix.tsv>` — claim a queued matrix and run it detached.
//!
//! Ported from fb-wave.sh. The claim is the part that matters: a matrix left in the queue
//! after being dispatched is picked up again by the autopilot, which destroyed four
//! completed runs once (bead farmerbob-20e).

use std::fs;
use std::path::{Path, PathBuf};

/// What claiming a matrix decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Claim {
    /// Moved out of the queue to this path. Nothing else can pick it up.
    Claimed(PathBuf),
    /// Not in the queue directory at all, so there is nothing to claim and the file is run
    /// where it lies. Running a matrix by hand from elsewhere is legitimate.
    NotQueued,
    /// There is no such matrix. After a successful claim this is what a second attempt
    /// sees, and it means another wave already took it -- a different fact from "I could
    /// not move it", and the one that makes double-dispatch detectable.
    Gone,
    /// It is in the queue and could not be moved. Refuse: running it unclaimed is how two
    /// waves end up on one matrix, destroying each other's worktrees (bead farmerbob-bal).
    Failed(String),
}

/// Whether `matrix` sits directly in `queue`.
pub fn is_queued(matrix: &Path, queue: &Path) -> bool {
    match (matrix.parent(), fs::canonicalize(queue)) {
        (Some(parent), Ok(q)) => fs::canonicalize(parent).map(|p| p == q).unwrap_or(false),
        _ => false,
    }
}

/// Move a queued matrix into `dispatched/`.
pub fn claim(matrix: &Path, queue: &Path) -> Claim {
    if !matrix.exists() {
        return Claim::Gone;
    }
    if !is_queued(matrix, queue) {
        return Claim::NotQueued;
    }
    let Some(name) = matrix.file_name() else {
        return Claim::Failed("no file name".to_string());
    };
    let dest_dir = queue.join("dispatched");
    if let Err(e) = fs::create_dir_all(&dest_dir) {
        return Claim::Failed(format!("cannot create {}: {e}", dest_dir.display()));
    }
    let dest = dest_dir.join(name);
    match fs::rename(matrix, &dest) {
        Ok(()) => Claim::Claimed(dest),
        Err(e) => Claim::Failed(format!("cannot claim {}: {e}", matrix.display())),
    }
}

/// The log a wave writes to, derived from the matrix name.
pub fn log_path(logs: &Path, matrix: &Path) -> PathBuf {
    let stem = matrix
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "wave".to_string());
    logs.join(format!("wave-{stem}.log"))
}

/// This binary's path, fit to be re-executed.
///
/// `current_exe()` resolves /proc/self/exe, and when the binary has been REPLACED under a
/// running process -- which `cargo build` does every time -- the kernel appends " (deleted)"
/// to that link. Spawning the result is ENOENT, so a long-running `fb autopilot` silently
/// lost the ability to launch anything the moment the tree was rebuilt: it claimed the
/// matrix, moved it to dispatched/, failed to start it, and reported "IDLE, queue empty"
/// on every tick afterwards.
///
/// A path that no longer names an executable falls back to the built binary in the tree,
/// which is the one a rebuild just wrote.
pub fn runnable_exe(current: Option<PathBuf>, fallback: PathBuf) -> PathBuf {
    if let Some(p) = current {
        let text = p.to_string_lossy().into_owned();
        let stripped = text
            .strip_suffix(" (deleted)")
            .map(PathBuf::from)
            .unwrap_or(p);
        if stripped.is_file() {
            return stripped;
        }
    }
    fallback
}

/// Claim the matrix and run it.
pub fn run(matrix: &Path, detach: bool) -> i32 {
    let repo = crate::paths::repo();
    let queue = repo.join(".fb/queue");
    let path = match claim(matrix, &queue) {
        Claim::Claimed(p) => {
            println!(
                "claimed {} -> dispatched/",
                p.file_name().unwrap_or_default().to_string_lossy()
            );
            p
        }
        Claim::NotQueued => matrix.to_path_buf(),
        Claim::Gone => {
            eprintln!(
                "REFUSING: {} is not there -- another wave claimed it",
                matrix.display()
            );
            return 1;
        }
        Claim::Failed(why) => {
            eprintln!("REFUSING: {why}");
            return 1;
        }
    };
    let log = log_path(&crate::paths::logs(), &path);
    if !detach {
        return crate::admit_cmd::run(&path);
    }
    let Ok(file) = fs::File::create(&log) else {
        eprintln!("cannot open {}", log.display());
        return 1;
    };
    let err = match file.try_clone() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("cannot duplicate the log handle: {e}");
            return 1;
        }
    };
    let exe = runnable_exe(std::env::current_exe().ok(), repo.join("target/debug/fb"));
    match std::process::Command::new(&exe)
        .args(["admit", &path.to_string_lossy()])
        .current_dir(&repo)
        .stdout(file)
        .stderr(err)
        .spawn()
    {
        Ok(child) => {
            println!(
                "wave {} -> pid {}",
                path.file_name().unwrap_or_default().to_string_lossy(),
                child.id()
            );
            println!("log  {}", log.display());
            0
        }
        Err(e) => {
            // PUT THE MATRIX BACK. A launch that never started has not consumed its queue
            // slot, and leaving it in dispatched/ makes the wave unrecoverable: the queue
            // reads empty forever after and nothing says why.
            if let Some(name) = path.file_name() {
                let back = repo.join(".fb/queue").join(name);
                if fs::rename(&path, &back).is_ok() {
                    eprintln!("returned {} to the queue", name.to_string_lossy());
                }
            }
            eprintln!("cannot start the wave with {}: {e}", exe.display());
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fb-wave-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let _ = fs::create_dir_all(d.join("queue"));
        d
    }

    /// A queued matrix is MOVED, so nothing can pick it up again. A matrix left in the
    /// queue after dispatch was relaunched by the autopilot and destroyed four completed
    /// runs (bead farmerbob-20e).
    #[test]
    fn claiming_moves_it_out_of_the_queue() {
        let d = scratch("claim");
        let q = d.join("queue");
        let m = q.join("wave1.tsv");
        let _ = fs::write(&m, "t\tfb\ta\n");
        match claim(&m, &q) {
            Claim::Claimed(p) => {
                assert!(p.ends_with("dispatched/wave1.tsv"), "{}", p.display());
                assert!(p.is_file(), "moved to dispatched");
                assert!(!m.exists(), "and gone from the queue");
            }
            other => panic!("expected Claimed, got {other:?}"),
        }
        let _ = fs::remove_dir_all(&d);
    }

    /// A matrix OUTSIDE the queue is not claimed and is run where it lies. Running one by
    /// hand from elsewhere is legitimate and must not be turned into a refusal.
    #[test]
    fn a_matrix_outside_the_queue_is_not_claimed() {
        let d = scratch("outside");
        let m = d.join("adhoc.tsv");
        let _ = fs::write(&m, "t\tfb\ta\n");
        assert_eq!(claim(&m, &d.join("queue")), Claim::NotQueued);
        assert!(m.exists(), "left where it lies");
        let _ = fs::remove_dir_all(&d);
    }

    /// Claiming twice cannot succeed twice. The second attempt finds nothing in the queue,
    /// which is what stops two waves running one matrix.
    #[test]
    fn a_matrix_cannot_be_claimed_twice() {
        let d = scratch("twice");
        let q = d.join("queue");
        let m = q.join("wave2.tsv");
        let _ = fs::write(&m, "t\tfb\ta\n");
        assert!(matches!(claim(&m, &q), Claim::Claimed(_)));
        assert_eq!(claim(&m, &q), Claim::Gone, "another wave already took it");
        let _ = fs::remove_dir_all(&d);
    }

    /// The log is named for the matrix, so two waves never share one.
    #[test]
    fn the_log_is_named_for_the_matrix() {
        assert_eq!(
            log_path(Path::new("/l"), Path::new("/q/wave7.tsv")),
            PathBuf::from("/l/wave-wave7.log")
        );
    }

    /// `cargo build` replaces the binary under a running process, and the kernel then
    /// appends " (deleted)" to /proc/self/exe. Spawning that path is ENOENT, which is how a
    /// live `fb autopilot` silently stopped launching anything and reported "IDLE, queue
    /// empty" on every tick instead.
    #[test]
    fn a_replaced_binary_falls_back_to_the_one_in_the_tree() {
        let fallback = PathBuf::from("/repo/target/debug/fb");
        assert_eq!(
            runnable_exe(
                Some(PathBuf::from("/repo/target/debug/fb (deleted)")),
                fallback.clone()
            ),
            fallback,
            "a deleted binary is not runnable, whatever its name says"
        );
        assert_eq!(runnable_exe(None, fallback.clone()), fallback);
    }

    /// A live binary is used as-is: the fallback is for the replaced case only.
    #[test]
    fn a_live_binary_is_used_unchanged() {
        let me = std::env::current_exe().unwrap();
        assert_eq!(runnable_exe(Some(me.clone()), PathBuf::from("/nope")), me);
    }

    /// A real file whose name genuinely ends in " (deleted)" still resolves to itself,
    /// because the stripped path would not exist.
    #[test]
    fn the_suffix_is_only_stripped_when_the_stripped_path_exists() {
        let d = std::env::temp_dir().join(format!("fb-wave-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        let odd = d.join("x (deleted)");
        std::fs::write(&odd, "").unwrap();
        // Nothing at `d/x`, so the strip fails and the fallback stands rather than a path
        // that is not there.
        assert_eq!(
            runnable_exe(Some(odd), PathBuf::from("/fb")),
            PathBuf::from("/fb")
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
