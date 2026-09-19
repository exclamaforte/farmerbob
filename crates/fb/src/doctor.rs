//! `fb doctor` — environment and repository preflight checks.
//!
//! Verifies that the current machine can actually run farmerbob: cgroup v2
//! delegation for confining agents, systemd, an NVIDIA GPU for serialized
//! inference, git worktree support, disk headroom and the presence of the
//! agent CLIs farmerbob drives in parallel.
//!
//! It also verifies that the REPOSITORY itself is in a state the harness can trust: no
//! pending `cargo fmt` diffs sitting undetected, no arm the registry calls dispatchable that
//! `fb-dispatch.sh` cannot actually launch, and no queued task spec whose declared deliverable
//! is already stale. All three were real outages on 2026-09-17, and none of them was a
//! property of the machine -- `check_cgroup_v2` and friends could not have caught any of
//! them, which is why they are checked here separately.

use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use farmerbob_core::orphan_check::{self, CrateSources};
use serde::Serialize;

/// Outcome of a single environment check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Status {
    /// The machine satisfies this check.
    Ok,
    /// The check is satisfiable but currently suboptimal.
    Warn,
    /// The machine cannot run farmerbob because of this check.
    Fail,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

/// A single named check result with an optional remediation command.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    /// Human-readable name of the check, e.g. `"cgroup v2"`.
    pub name: &'static str,
    /// Whether the machine passes, nearly passes, or fails this check.
    pub status: Status,
    /// Human-readable detail about what was found.
    pub message: String,
    /// An optional command the user can run to fix the problem.
    pub fix: Option<String>,
}

/// Agent CLIs farmerbob expects to find on `PATH`.
const AGENTS: &[&str] = &["claude", "codex", "zcode", "agy", "opencode", "ori"];

/// Warn when less than this many bytes are free on the home filesystem.
const DISK_WARN_BYTES: u64 = 10 * 1024 * 1024 * 1024;

/// Minimum git version that supports `git worktree`.
const GIT_WORKTREE_MINOR: u32 = 5;

/// Controllers farmerbob requires to be delegated to the user slice.
const REQUIRED_CONTROLLERS: &[&str] = &["cpu", "memory", "pids"];

/// Path of the systemd unit override used to delegate controllers.
const DELEGATE_CONF: &str = "/etc/systemd/system/user@.service.d/delegate.conf";

/// Run all preflight checks.
///
/// With `json` set, prints a JSON array of checks instead of a table.
/// Returns the process exit code: `0` when no check failed, `1` otherwise.
pub fn run(json: bool) -> i32 {
    let checks = run_all();
    if json {
        match serde_json::to_string_pretty(&checks) {
            Ok(text) => println!("{text}"),
            Err(e) => eprintln!("could not serialize doctor report: {e}"),
        }
    } else {
        print_table(&checks);
    }
    if checks.iter().any(|c| c.status == Status::Fail) {
        1
    } else {
        0
    }
}

/// Build the full report by running every check.
fn run_all() -> Vec<Check> {
    let repo = crate::paths::repo();
    let scanned = scan_crate_sources(&repo.join("crates"));
    vec![
        check_cgroup_v2(),
        check_delegated_controllers(),
        check_systemd(),
        check_nvidia(),
        check_git_worktree(),
        check_disk_headroom(),
        check_agent_clis(),
        check_rustfmt(&repo),
        check_launcher_coverage(&repo),
        check_spec_targets(&repo),
        decide_orphaned_modules(&orphan_sources(&scanned)),
        decide_unreachable_modules(&scanned),
    ]
}

/// Check that every directly-listed Rust source file belongs to the crate's module tree.
///
/// `run_all` scans once and calls the decide functions directly with the shared scan, so
/// this filesystem-in wrapper has only test callers and is compiled for them only.
#[cfg(test)]
fn check_orphaned_modules(repo: &Path) -> Check {
    let crates = scan_crate_sources(&repo.join("crates"));
    decide_orphaned_modules(&orphan_sources(&crates))
}

/// Turn core's orphan decisions into the doctor report without scanning the filesystem.
fn decide_orphaned_modules(crates: &[CrateSources]) -> Check {
    let orphans = orphan_check::orphans(crates);
    if orphans.is_empty() {
        let message = if crates.is_empty() {
            "no crates were examined".to_string()
        } else {
            format!(
                "no orphaned modules found across {}",
                count_noun(crates.len(), "crate")
            )
        };
        return Check::new("orphaned modules", Status::Ok, message, None);
    }

    let details = orphans
        .iter()
        .map(|orphan| format!("{}:{}", orphan.krate, orphan.stem))
        .collect::<Vec<_>>();
    let fixes = orphans
        .iter()
        .map(|orphan| format!("{}: mod {};", orphan.krate, orphan.stem))
        .collect::<Vec<_>>();
    Check::new(
        "orphaned modules",
        Status::Fail,
        format!("orphaned modules: {}", details.join(", ")),
        Some(format!("add module declarations: {}", fixes.join(", "))),
    )
}

/// Turn a crate scan into the unreachable-modules report, without touching the filesystem.
///
/// A module of a BINARY crate (one with a `main.rs` and no `lib.rs`) is reachable when
/// `main.rs` names it as `<stem>::`, or when a module itself reachable names it that way;
/// reachability is closed transitively from `main.rs`. A self-reference (`foo.rs` naming
/// `foo::`) cannot seed the closure, and a cycle of modules naming each other stays
/// unreachable unless something already reachable names one of them. A crate with a
/// `lib.rs` contributes nothing to the report: its `pub mod`s are reachable by definition
/// of an external caller, and rather than guess which private modules a binary half also
/// reaches, the whole crate is out of this check's claim. A crate with neither root is
/// likewise skipped (unpinned by the specification). Roots themselves are never reported.
///
/// `Status` is `Warn` when anything is found -- unreachable code still compiles and its
/// tests still run; it is work that is not wired, not a machine that cannot run farmerbob.
/// Deliberately unlike the orphan check's `Fail`.
fn decide_unreachable_modules(crates: &[ScannedCrate]) -> Check {
    let mut unreachable = Vec::new();
    for krate in crates {
        let Some(main) = &krate.sources.main_rs else {
            continue;
        };
        if krate.sources.lib_rs.is_some() {
            continue;
        }
        let reachable = reachable_stems(main, &krate.bodies);
        for (stem, _) in &krate.bodies {
            if !reachable.iter().any(|r| r == stem) {
                unreachable.push(format!("{}:{stem}", krate.sources.name));
            }
        }
    }

    if unreachable.is_empty() {
        let message = if crates.is_empty() {
            "no crates were examined".to_string()
        } else {
            format!(
                "no unreachable modules found across {}",
                count_noun(crates.len(), "crate")
            )
        };
        return Check::new("unreachable modules", Status::Ok, message, None);
    }

    Check::new(
        "unreachable modules",
        Status::Warn,
        format!("unreachable modules: {}", unreachable.join(", ")),
        Some(format!(
            "wire each into a live call path from main.rs, or delete it: {}",
            unreachable.join(", ")
        )),
    )
}

/// The stems reachable from a binary crate's `main.rs`, by transitive closure over
/// textual `<stem>::` mentions.
///
/// The scan is textual: a mention inside a comment or a string literal counts the same as
/// one in live code. That over-reports reachability, which is the safe direction -- the
/// check stays silent about modules it only thinks are used, rather than naming live code
/// as dead.
fn reachable_stems(main: &str, bodies: &[(String, String)]) -> Vec<String> {
    let mut reachable: Vec<String> = Vec::new();
    let mut stack: Vec<&(String, String)> = bodies
        .iter()
        .filter(|(stem, _)| names_as_path(main, stem))
        .collect();
    while let Some((stem, body)) = stack.pop() {
        reachable.push(stem.clone());
        for candidate in bodies {
            if reachable.iter().any(|r| r == &candidate.0)
                || stack.iter().any(|(s, _)| s == &candidate.0)
            {
                continue;
            }
            if names_as_path(body, &candidate.0) {
                stack.push(candidate);
            }
        }
    }
    reachable
}

/// Whether `text` mentions `stem` as a path prefix (`stem::`), at a word boundary.
fn names_as_path(text: &str, stem: &str) -> bool {
    text.match_indices(stem).any(|(at, _)| {
        let preceded = text[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        !preceded && text[at + stem.len()..].starts_with("::")
    })
}

/// One crate's sources as scanned from disk: the facts `farmerbob_core::orphan_check`
/// needs, plus the body text of every non-root module file, which the reachability check
/// needs to follow `<stem>::` mentions.
struct ScannedCrate {
    /// Stems and root texts, in the shape core's orphan API takes.
    sources: CrateSources,
    /// `(stem, body)` for every readable non-root `.rs` file directly in `src/`.
    bodies: Vec<(String, String)>,
}

/// The orphan-check view of a scan, with the module bodies stripped off.
fn orphan_sources(scanned: &[ScannedCrate]) -> Vec<CrateSources> {
    scanned.iter().map(|c| c.sources.clone()).collect()
}

/// Read the source facts needed by `farmerbob_core::orphan_check`, plus each module
/// file's body for the reachability check -- one directory walk serving both.
///
/// Only immediate children of `crates/` are crate candidates. A candidate without `src/`, a
/// directory that cannot be read, a non-UTF-8 crate name, or an unreadable root is skipped because
/// the core API has no representation for an unreadable source listing. A module body that
/// cannot be read is omitted from `bodies` only: its stem still stands for the orphan check,
/// and it counts as reachable by nothing (the safe direction for orphans, the report-prone
/// direction for reachability -- a declared-but-unreadable module merits a warning).
fn scan_crate_sources(crates_dir: &Path) -> Vec<ScannedCrate> {
    let Ok(entries) = fs::read_dir(crates_dir) else {
        return Vec::new();
    };
    let mut crates = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let crate_dir = entry.path();
        if !crate_dir.is_dir() {
            continue;
        }
        let src_dir = crate_dir.join("src");
        if !src_dir.is_dir() {
            continue;
        }
        let Some(name) = crate_dir.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(stems) = source_stems(&src_dir) else {
            continue;
        };
        let Some(lib_rs) = root_source(&src_dir.join("lib.rs")) else {
            continue;
        };
        let Some(main_rs) = root_source(&src_dir.join("main.rs")) else {
            continue;
        };
        let mut bodies = Vec::new();
        for stem in &stems {
            if stem == "main" || stem == "lib" {
                continue;
            }
            if let Ok(body) = fs::read_to_string(src_dir.join(format!("{stem}.rs"))) {
                bodies.push((stem.clone(), body));
            }
        }
        crates.push(ScannedCrate {
            sources: CrateSources {
                name: name.to_string(),
                stems,
                lib_rs,
                main_rs,
            },
            bodies,
        });
    }
    crates.sort_by(|left, right| left.sources.name.cmp(&right.sources.name));
    crates
}

/// Return the stems of `.rs` files directly inside one `src/` directory.
fn source_stems(src_dir: &Path) -> Option<Vec<String>> {
    let entries = fs::read_dir(src_dir).ok()?;
    let mut stems = Vec::new();
    for entry in entries {
        let entry = entry.ok()?;
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let stem = path.file_stem()?.to_str()?.to_string();
        stems.push(stem);
    }
    stems.sort();
    Some(stems)
}

/// Read an optional crate root, distinguishing a missing root from an unreadable one.
fn root_source(path: &Path) -> Option<Option<String>> {
    match fs::read_to_string(path) {
        Ok(contents) => Some(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(None),
        Err(_) => None,
    }
}

/// Print the checks as a human-readable table.
fn print_table(checks: &[Check]) {
    let header = format!("{:<24} {:<6} {}", "check", "status", "message");
    println!("{header}\n{}", "-".repeat(88));
    for check in checks {
        let fix = check.fix.as_deref().unwrap_or("");
        println!("{:<24} {:<6} {}", check.name, check.status, check.message);
        if !fix.is_empty() {
            println!("{:<24} {:<6} fix: {fix}", "", "");
        }
    }
}

impl Check {
    /// Create a check result with an optional fix.
    fn new(
        name: &'static str,
        status: Status,
        message: impl Into<String>,
        fix: Option<String>,
    ) -> Self {
        Check {
            name,
            status,
            message: message.into(),
            fix,
        }
    }
}

/// Check that the kernel exposes cgroup v2 controllers.
fn check_cgroup_v2() -> Check {
    match fs::read_to_string("/sys/fs/cgroup/cgroup.controllers") {
        Ok(controllers) => {
            let list = controllers.split_whitespace().collect::<Vec<_>>().join(" ");
            Check::new(
                "cgroup v2",
                Status::Ok,
                format!("cgroup v2 active; root controllers: {list}"),
                None,
            )
        }
        Err(e) => Check::new(
            "cgroup v2",
            Status::Fail,
            format!("/sys/fs/cgroup/cgroup.controllers: {e}"),
            Some(
                "boot with a cgroup v2 kernel: add systemd.unified_cgroup_hierarchy=1 \
                 to the kernel command line and reboot"
                    .to_string(),
            ),
        ),
    }
}

/// Check that `cpu`, `memory` and `pids` are delegated to the user slice.
fn check_delegated_controllers() -> Check {
    let Some(uid) = current_uid() else {
        return Check::new(
            "delegated controllers",
            Status::Fail,
            "could not determine the current uid",
            None,
        );
    };
    let path = PathBuf::from(format!(
        "/sys/fs/cgroup/user.slice/user-{uid}.slice/cgroup.controllers"
    ));
    let contents = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            return Check::new(
                "delegated controllers",
                Status::Fail,
                format!("{}: {e}", path.display()),
                Some(delegate_fix()),
            );
        }
    };
    let missing = missing_controllers(&contents, REQUIRED_CONTROLLERS);
    if !missing.is_empty() {
        return Check::new(
            "delegated controllers",
            Status::Fail,
            format!(
                "controllers not delegated to your cgroup: {}",
                missing.join(", ")
            ),
            Some(delegate_fix()),
        );
    }
    if !parse_controllers(&contents).contains(&"cpuset") {
        return Check::new(
            "delegated controllers",
            Status::Warn,
            "cpuset is not delegated, so CPU/GPU affinity confinement is unavailable",
            Some(delegate_fix()),
        );
    }
    Check::new(
        "delegated controllers",
        Status::Ok,
        "cpu, memory and pids are delegated (and cpuset is available)",
        None,
    )
}

/// Check that `systemd-run` exists and runs.
fn check_systemd() -> Check {
    match Command::new("systemd-run").arg("--version").output() {
        Ok(out) if out.status.success() => {
            let version = String::from_utf8_lossy(&out.stdout);
            let first = version.lines().next().unwrap_or("").trim();
            Check::new(
                "systemd",
                Status::Ok,
                format!("systemd-run available: {first}"),
                None,
            )
        }
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            Check::new(
                "systemd",
                Status::Fail,
                format!(
                    "systemd-run --version exited {}: {}",
                    out.status,
                    err.trim()
                ),
                Some("install systemd".to_string()),
            )
        }
        Err(e) => Check::new(
            "systemd",
            Status::Fail,
            format!("systemd-run could not be executed: {e}"),
            Some("install systemd and log in through a systemd-managed session".to_string()),
        ),
    }
}

/// Check that `nvidia-smi` works and report the GPU and driver version.
fn check_nvidia() -> Check {
    let output = match Command::new("nvidia-smi")
        .args(["--query-gpu=name,driver_version", "--format=csv,noheader"])
        .output()
    {
        Ok(out) => out,
        Err(e) => {
            return Check::new(
                "nvidia",
                Status::Warn,
                format!("nvidia-smi not found: {e}"),
                Some("install the NVIDIA driver (provides nvidia-smi) to use the GPU".to_string()),
            );
        }
    };
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Check::new(
            "nvidia",
            Status::Warn,
            format!("nvidia-smi exited {}: {}", output.status, err.trim()),
            Some("install the NVIDIA driver (provides nvidia-smi) to use the GPU".to_string()),
        );
    }
    let text = String::from_utf8_lossy(&output.stdout);
    match text.lines().next().and_then(parse_gpu_line) {
        Some((name, driver)) => Check::new(
            "nvidia",
            Status::Ok,
            format!("{name} (driver {driver})"),
            None,
        ),
        None => Check::new(
            "nvidia",
            Status::Warn,
            format!("nvidia-smi returned no GPU info: {}", text.trim()),
            None,
        ),
    }
}

/// Check that git is new enough to support `git worktree` (>= 2.5).
fn check_git_worktree() -> Check {
    let output = match Command::new("git").arg("--version").output() {
        Ok(out) => out,
        Err(e) => {
            return Check::new(
                "git worktree",
                Status::Fail,
                format!("git not found: {e}"),
                Some("install git 2.5 or newer".to_string()),
            );
        }
    };
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    match parse_git_version(&version) {
        Some((major, minor)) if (major, minor) >= (2, GIT_WORKTREE_MINOR) => Check::new(
            "git worktree",
            Status::Ok,
            format!("git {major}.{minor} supports worktree"),
            None,
        ),
        Some((major, minor)) => Check::new(
            "git worktree",
            Status::Fail,
            format!(
                "git {major}.{minor} is too old; worktree requires >= 2.{}",
                GIT_WORKTREE_MINOR
            ),
            Some("upgrade git to 2.5 or newer".to_string()),
        ),
        None => Check::new(
            "git worktree",
            Status::Fail,
            format!("could not parse git version: {version}"),
            None,
        ),
    }
}

/// Check for at least 10 GiB of free space on the home filesystem.
fn check_disk_headroom() -> Check {
    let home = home_dir();
    let home_str = home.display().to_string();
    let output = match Command::new("df").args(["-kP", &home_str]).output() {
        Ok(out) => out,
        Err(e) => {
            return Check::new(
                "disk headroom",
                Status::Fail,
                format!("df not found: {e}"),
                None,
            );
        }
    };
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Check::new(
            "disk headroom",
            Status::Fail,
            format!("df failed: {}", err.trim()),
            None,
        );
    }
    let text = String::from_utf8_lossy(&output.stdout);
    match text.lines().skip(1).find_map(parse_df_available_kb) {
        Some(kb) => {
            let free = kb * 1024;
            if free < DISK_WARN_BYTES {
                Check::new(
                    "disk headroom",
                    Status::Warn,
                    format!(
                        "only {} free on {}; at least 10 GiB is recommended",
                        human_bytes(free),
                        home.display()
                    ),
                    Some(
                        "free up disk space or move the worktree to a larger filesystem"
                            .to_string(),
                    ),
                )
            } else {
                Check::new(
                    "disk headroom",
                    Status::Ok,
                    format!("{} free on {}", human_bytes(free), home.display()),
                    None,
                )
            }
        }
        None => Check::new(
            "disk headroom",
            Status::Fail,
            format!("could not parse df output: {text}"),
            None,
        ),
    }
}

/// Check that the agent CLIs farmerbob drives are on `PATH`.
fn check_agent_clis() -> Check {
    let path = env::var_os("PATH").unwrap_or_default();
    let path = path.to_string_lossy();
    let mut found = Vec::new();
    let mut missing = Vec::new();
    for agent in AGENTS {
        if find_on_path(&path, agent).is_some() {
            found.push(*agent);
        } else {
            missing.push(*agent);
        }
    }
    let report = AGENTS
        .iter()
        .map(|a| {
            format!(
                "{a}: {}",
                if found.contains(a) {
                    "found"
                } else {
                    "missing"
                }
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    if missing.is_empty() {
        Check::new("agent CLIs", Status::Ok, report, None)
    } else {
        Check::new(
            "agent CLIs",
            Status::Warn,
            format!("{report}; install: {}", missing.join(", ")),
            Some(format!(
                "install missing agent CLIs: {}",
                missing.join(", ")
            )),
        )
    }
}

// ---------------------------------------------------------------- repository checks
//
// Everything above checks the MACHINE. Everything below checks the REPOSITORY: whether it is
// clean, whether the registry and the dispatcher agree about which arms can run, and whether
// queued task specs are still actionable. `repo` is the repository root, passed in rather
// than discovered here, so these are testable against a throwaway fixture directory instead
// of the real checkout.

/// `cargo fmt --all --check` over the workspace rooted at `repo`.
///
/// An unrunnable check has not passed: this never returns `Status::Ok` when `cargo fmt`
/// could not be run to completion, whether because `cargo` itself could not be spawned or
/// because it exited for a reason other than "there are pending diffs" (a missing
/// `Cargo.toml`, for instance). A repository with zero `.rs` files is `Ok`, not `Warn` --
/// nothing to format is not a failure to format -- and is decided without invoking `cargo` at
/// all, so a fixture with no Cargo workspace can still exercise that boundary.
fn check_rustfmt(repo: &Path) -> Check {
    if !repo.is_dir() {
        return Check::new(
            "rustfmt",
            Status::Warn,
            format!("{}: not a directory", repo.display()),
            None,
        );
    }
    let rs_files = count_rs_files(repo);
    if rs_files == 0 {
        return Check::new("rustfmt", Status::Ok, "no .rs files to format", None);
    }

    let output = Command::new("cargo")
        .args(["fmt", "--all", "--check", "--", "--files-with-diff"])
        .current_dir(repo)
        .output();
    let output = match output {
        Ok(o) => o,
        Err(e) => {
            return Check::new(
                "rustfmt",
                Status::Warn,
                format!("cargo fmt could not be run: {e}"),
                None,
            );
        }
    };
    if output.status.success() {
        return Check::new(
            "rustfmt",
            Status::Ok,
            format!(
                "no pending rustfmt diffs across {}",
                count_noun(rs_files, "file")
            ),
            None,
        );
    }

    // `--files-with-diff` prints one path per line for each file that would change and
    // nothing else, so the line count IS the file count -- counting diff hunks instead is
    // exactly the mistake that hid 489 hunks across 53 files for a year.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let dirty = stdout.lines().filter(|l| !l.trim().is_empty()).count();
    if dirty > 0 {
        return Check::new(
            "rustfmt",
            Status::Warn,
            format!("{} pending rustfmt diffs", count_noun(dirty, "file")),
            Some("cargo fmt --all".to_string()),
        );
    }

    // Nonzero exit, but no file list: cargo failed before rustfmt ran at all (no
    // `Cargo.toml`, no `cargo`/`rustfmt` toolchain, ...). That is still "could not run",
    // never "Ok".
    let stderr = String::from_utf8_lossy(&output.stderr);
    Check::new(
        "rustfmt",
        Status::Warn,
        format!("cargo fmt --all --check could not run: {}", stderr.trim()),
        None,
    )
}

/// Every arm `sources.toml` marks dispatchable must have a launcher branch in `fb-dispatch.sh`.
///
/// "Dispatchable" excludes an arm whose `status` is `"disabled"` and one carrying a
/// `redundant_with` key (clause 6): neither is ever launched, so neither belongs in the
/// launcher table and its absence there is not a finding. Coverage is decided by actually
/// parsing `fb-dispatch.sh`'s `case "$SRC" in ... esac` block rather than by a hardcoded
/// guess at what it should contain -- a hardcoded guess is exactly how `claude-sonnet` passed
/// eligibility and then failed to launch, since the registry and the launcher table are two
/// independently-edited copies of the same fact. The parser understands a KNOWN SUBSET of
/// bash `case` patterns: an exact literal, and a trailing-wildcard prefix like `"ifm-*"`. The
/// bare catch-all `"*"` is recognized and deliberately excluded from coverage, because in
/// `fb-dispatch.sh` it is the "unknown source" error branch, not a launcher. Any arm this
/// parser cannot classify against a real pattern is reported as missing rather than assumed
/// covered.
///
/// A missing or unreadable `sources.toml` or `fb-dispatch.sh` is `Warn`, naming which file
/// could not be read -- never `Ok`, for the same reason as `check_rustfmt`. A `sources.toml`
/// with no dispatchable arms is `Ok`. An arm that is dispatchable and absent from the table is
/// `Fail`: this is the fault that cost a whole run, so it is scored higher than a warning.
fn check_launcher_coverage(repo: &Path) -> Check {
    let sources_path = repo.join("sources.toml");
    let sources_text = match fs::read_to_string(&sources_path) {
        Ok(t) => t,
        Err(e) => {
            return Check::new(
                "launcher coverage",
                Status::Warn,
                format!("{}: {e}", sources_path.display()),
                None,
            );
        }
    };
    let dispatch_path = repo.join("crates/fb/src/launch.rs");
    let dispatch_text = match fs::read_to_string(&dispatch_path) {
        Ok(t) => t,
        Err(e) => {
            return Check::new(
                "launcher coverage",
                Status::Warn,
                format!("{}: {e}", dispatch_path.display()),
                None,
            );
        }
    };
    let doc: toml::Value = match toml::from_str(&sources_text) {
        Ok(v) => v,
        Err(e) => {
            return Check::new(
                "launcher coverage",
                Status::Warn,
                format!("{}: not valid TOML: {e}", sources_path.display()),
                None,
            );
        }
    };

    let dispatchable = dispatchable_arms(&doc);
    let rules = classify_patterns(&dispatch_case_patterns(&dispatch_text));
    let missing = uncovered_arms(&dispatchable, &rules);

    if missing.is_empty() {
        Check::new(
            "launcher coverage",
            Status::Ok,
            format!(
                "{} dispatchable in sources.toml, every one has a launcher branch in launch.rs",
                count_noun(dispatchable.len(), "arm")
            ),
            None,
        )
    } else {
        Check::new(
            "launcher coverage",
            Status::Fail,
            format!(
                "{} dispatchable but missing a launcher branch in fb-dispatch.sh: {}",
                count_noun(missing.len(), "arm"),
                missing.join(", ")
            ),
            None,
        )
    }
}

/// Every spec in `.fb/prompts` declares a target, and an `fb:creates` target must not exist.
///
/// A spec "declares a target" by carrying at least one `<!-- fb:creates PATH -->` or
/// `<!-- fb:modifies PATH -->` comment; a spec with neither is flagged as declaring no target.
/// Separately, every `fb:creates` path a spec DOES declare is checked against `repo`: if it
/// already exists, that spec cannot succeed (every arm correctly no-ops, and the harness
/// scores the whole no-op wave as failure), so it is flagged too. These are different faults
/// with different causes -- one is a spec nobody finished writing, the other is a spec the
/// repository outgrew -- and are reported in separate, separately-named lists so a reader is
/// never left guessing which one they are looking at.
///
/// An empty (or missing) `.fb/prompts` directory is `Ok`: every spec in an empty set trivially
/// declares a target and no `fb:creates` target in an empty set exists. Only `.md` files are
/// treated as specs.
fn check_spec_targets(repo: &Path) -> Check {
    let files = spec_md_files(&repo.join(".fb/prompts"));

    let mut no_target = Vec::new();
    let mut stale = Vec::new();
    for path in &files {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("<unnamed>")
            .to_string();
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let targets = parse_spec_targets(&text);
        if targets.creates.is_empty() && targets.modifies.is_empty() {
            no_target.push(name.clone());
        }
        for created in &targets.creates {
            if !Path::new(created).is_absolute() && repo.join(created).exists() {
                stale.push(format!("{name} -> {created}"));
            }
        }
    }

    if no_target.is_empty() && stale.is_empty() {
        return Check::new(
            "spec targets",
            Status::Ok,
            format!(
                "{} in .fb/prompts, all declare a target",
                count_noun(files.len(), "spec")
            ),
            None,
        );
    }

    let mut parts = Vec::new();
    if !stale.is_empty() {
        parts.push(format!(
            "{} whose fb:creates target already exists: {}",
            count_noun(stale.len(), "spec"),
            stale.join(", ")
        ));
    }
    if !no_target.is_empty() {
        parts.push(format!(
            "{} with no fb:creates/fb:modifies target: {}",
            count_noun(no_target.len(), "spec"),
            no_target.join(", ")
        ));
    }
    Check::new("spec targets", Status::Warn, parts.join("; "), None)
}

/// Read the current uid from `/proc/self/status`.
fn current_uid() -> Option<u32> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    parse_uid_from_status(&status)
}

/// Parse the real uid from `/proc/self/status` contents (the `Uid:` line).
fn parse_uid_from_status(contents: &str) -> Option<u32> {
    contents.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        if parts.next()? == "Uid:" {
            parts.next()?.parse().ok()
        } else {
            None
        }
    })
}

/// Parse the controller names from a `cgroup.controllers` file.
fn parse_controllers(contents: &str) -> Vec<&str> {
    contents.split_whitespace().collect()
}

/// Return the required controllers missing from a `cgroup.controllers` file.
fn missing_controllers<'a>(contents: &str, required: &[&'a str]) -> Vec<&'a str> {
    let present = parse_controllers(contents);
    required
        .iter()
        .copied()
        .filter(|c| !present.contains(c))
        .collect()
}

/// The fix for missing delegated controllers.
fn delegate_fix() -> String {
    format!(
        "sudo mkdir -p /etc/systemd/system/user@.service.d && \
         echo '[Service]\nDelegate=cpu cpuset io memory pids' | \
         sudo tee {DELEGATE_CONF} && sudo systemctl daemon-reexec"
    )
}

/// Parse `major.minor` from a `git --version` string.
fn parse_git_version(version: &str) -> Option<(u32, u32)> {
    let major_start = version.find(|c: char| c.is_ascii_digit())?;
    let after = &version[major_start..];
    let major_end = after.find('.')?;
    let major = after[..major_end].parse::<u32>().ok()?;
    let minor = after[major_end + 1..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>();
    if minor.is_empty() {
        return None;
    }
    Some((major, minor.parse().ok()?))
}

/// Parse the free-space column (in 1K blocks) from a `df -kP` data line.
fn parse_df_available_kb(line: &str) -> Option<u64> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 5 || !fields[4].ends_with('%') {
        return None;
    }
    fields[3].parse::<u64>().ok()
}

/// Format a byte count in human-readable units.
fn human_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = UNITS[0];
    for next in &UNITS[1..] {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = next;
    }
    format!("{value:.1} {unit}")
}

/// Determine the user's home directory.
fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Locate `name` as an executable file on the given `PATH`.
fn find_on_path(path: &str, name: &str) -> Option<PathBuf> {
    for dir in split_path(path) {
        let dir = if dir.is_empty() { "." } else { dir };
        let candidate = Path::new(dir).join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// Split a `PATH` string into its directories (empty entries mean `.`).
fn split_path(path: &str) -> Vec<&str> {
    path.split(':').collect()
}

/// Whether the file exists and has any execute bit set.
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    if !path.is_file() {
        return false;
    }
    path.metadata()
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Parse a `name,driver_version` CSV line from `nvidia-smi`.
fn parse_gpu_line(line: &str) -> Option<(String, String)> {
    let mut parts = line.splitn(2, ',');
    let name = parts.next()?.trim();
    let driver = parts.next()?.trim();
    if name.is_empty() || driver.is_empty() {
        return None;
    }
    Some((name.to_string(), driver.to_string()))
}

// ---- repository-check helpers -------------------------------------------------------

/// Format a count with an English noun, pluralizing with a trailing `s` when `n != 1`.
///
/// Every noun these checks count (`file`, `arm`, `spec`) pluralizes this way, so this does
/// not attempt anything beyond it.
fn count_noun(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// Count `.rs` files anywhere under `dir`, skipping `target` (build output) and any directory
/// whose name starts with `.` (VCS and tool metadata) -- neither holds a source file `cargo
/// fmt` would ever touch. Unreadable directories contribute zero rather than erroring, since
/// this is a cheap existence probe, not the check itself.
fn count_rs_files(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut count = 0;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == "target" || name.starts_with('.') {
                continue;
            }
            count += count_rs_files(&path);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            count += 1;
        }
    }
    count
}

/// Arm names in `sources.toml`'s `[source.*]` tables that farmerbob may actually dispatch to.
///
/// An arm is dispatchable unless its `status` is exactly `"disabled"`, or it carries a
/// `redundant_with` key -- a live route to the same model exists elsewhere, so this route's
/// absence from the launcher table is not itself a finding (clause 6). A missing `status` key
/// does not disqualify an arm; only an explicit `"disabled"` does. A document with no
/// `[source]` table produces an empty vec, the same as a `[source]` table with no entries.
/// Returned sorted lexicographically by arm name.
fn dispatchable_arms(doc: &toml::Value) -> Vec<String> {
    let Some(table) = doc.get("source").and_then(toml::Value::as_table) else {
        return Vec::new();
    };
    let mut names: Vec<String> = table
        .iter()
        .filter(|(_, arm)| {
            let disabled = arm.get("status").and_then(|s| s.as_str()) == Some("disabled");
            let redundant = arm.get("redundant_with").is_some();
            !disabled && !redundant
        })
        .map(|(name, _)| name.clone())
        .collect();
    names.sort();
    names
}

/// Extract the raw `case` patterns from the `case "$SRC" in ... esac` block in
/// `fb-dispatch.sh` that decides how each arm is launched.
///
/// A pattern group like `ifm-*)` or `codex-luna)` is recognized only at the start of a
/// (trimmed) line, immediately followed by `)`, which is how bash `case` arms are written and
/// is what keeps this from matching `)` characters that show up inside the launcher commands
/// themselves. Each group is split on `|` and every non-empty token is returned, in source
/// order. Returns an empty vec if no `case "$SRC" in` block is found -- `check_launcher_coverage`
/// treats that as "no arm has launcher coverage", not as "every arm is covered".
fn dispatch_case_patterns(script: &str) -> Vec<String> {
    const HEADER: &str = "case \"$SRC\" in";
    let mut patterns = Vec::new();
    let mut in_block = false;
    for line in script.lines() {
        let trimmed = line.trim();
        if !in_block {
            if trimmed == HEADER {
                in_block = true;
            }
            continue;
        }
        if trimmed == "esac" {
            break;
        }
        let Some(paren) = trimmed.find(')') else {
            continue;
        };
        let head = &trimmed[..paren];
        let is_pattern_charset = !head.is_empty()
            && head
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '*' | '|'));
        if !is_pattern_charset {
            continue;
        }
        patterns.extend(
            head.split('|')
                .filter(|t| !t.is_empty())
                .map(str::to_string),
        );
    }
    patterns
}

/// Coverage rules a `case` pattern set resolves to: which arm names it matches.
struct CoverageRules {
    /// Patterns that must match an arm name exactly.
    exact: Vec<String>,
    /// Patterns that match any arm name starting with this (non-empty) prefix.
    prefix: Vec<String>,
}

/// Classify raw `case` patterns into exact-match and prefix-match coverage rules.
///
/// This recognizes a KNOWN SUBSET of bash glob syntax: a literal token becomes an exact-match
/// rule, and a token ending in `*` (e.g. `"ifm-*"`) becomes a prefix-match rule on everything
/// before the `*`. The bare catch-all `"*"` is dropped rather than turned into an
/// empty-string prefix that would match anything -- in `fb-dispatch.sh` it is the "unknown
/// source" error branch, so treating it as coverage would recreate exactly the bug this check
/// exists to catch. Any other shape (`*` embedded or leading, a character class, ...) is
/// outside this subset and is also dropped: an arm only reachable through such a pattern is
/// reported as uncovered by `uncovered_arms`, never silently assumed covered.
fn classify_patterns(patterns: &[String]) -> CoverageRules {
    let mut exact = Vec::new();
    let mut prefix = Vec::new();
    for p in patterns {
        if p == "*" {
            continue;
        }
        if let Some(stripped) = p.strip_suffix('*') {
            if !stripped.is_empty() && !stripped.contains('*') {
                prefix.push(stripped.to_string());
            }
            continue;
        }
        if !p.contains('*') {
            exact.push(p.clone());
        }
    }
    CoverageRules { exact, prefix }
}

/// Whether `arm` is matched by an exact or prefix rule in `rules`.
fn is_covered(arm: &str, rules: &CoverageRules) -> bool {
    rules.exact.iter().any(|e| e == arm) || rules.prefix.iter().any(|p| arm.starts_with(p.as_str()))
}

/// Dispatchable arms matched by neither an exact nor a prefix coverage rule.
///
/// Returned sorted lexicographically. An empty `dispatchable` slice always yields an empty
/// vec -- meaning "nothing to check", not "everything covered". `check_launcher_coverage`
/// tells the two apart itself, by looking at whether `dispatchable` was empty to begin with.
fn uncovered_arms(dispatchable: &[String], rules: &CoverageRules) -> Vec<String> {
    let mut out: Vec<String> = dispatchable
        .iter()
        .filter(|a| !is_covered(a, rules))
        .cloned()
        .collect();
    out.sort();
    out
}

/// What one `.fb/prompts/*.md` spec declares about its own deliverable.
struct SpecTargets {
    /// Paths named by `<!-- fb:creates PATH -->`, in file order.
    creates: Vec<String>,
    /// Paths named by `<!-- fb:modifies PATH -->`, in file order.
    modifies: Vec<String>,
}

/// Parse `fb:creates`/`fb:modifies` declarations out of a spec's markdown text.
///
/// Recognizes lines of the shape `<!-- fb:creates PATH -->` or `<!-- fb:modifies PATH -->`
/// (surrounding whitespace on the line, and around `PATH`, is tolerated). Both lists preserve
/// file order and may contain duplicates -- callers that care about uniqueness dedupe
/// themselves. A spec with neither verb yields two empty lists; that is the "no target
/// declared" case `check_spec_targets` reports.
fn parse_spec_targets(text: &str) -> SpecTargets {
    let mut creates = Vec::new();
    let mut modifies = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("<!--") else {
            continue;
        };
        let rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("fb:creates")
            && let Some(path) = after.trim_start().strip_suffix("-->")
            && !path.trim().is_empty()
        {
            creates.push(path.trim().to_string());
        } else if let Some(after) = rest.strip_prefix("fb:modifies")
            && let Some(path) = after.trim_start().strip_suffix("-->")
            && !path.trim().is_empty()
        {
            modifies.push(path.trim().to_string());
        }
    }
    SpecTargets { creates, modifies }
}

/// `.md` files directly inside `dir`, sorted by filename.
///
/// A missing or unreadable directory, and a readable directory with no `.md` entries, both
/// produce an empty vec: this function treats "nothing to examine" the same as "nothing
/// found", since either way there is nothing for `check_spec_targets` to flag.
fn spec_md_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("md"))
        .collect();
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uid_parses_real_uid_line() {
        let status = "Name:\tfb\nUid:\t1000\t1000\t1000\t1000\nGid:\t1000\n";
        assert_eq!(parse_uid_from_status(status), Some(1000));
    }

    #[test]
    fn uid_missing_returns_none() {
        assert_eq!(parse_uid_from_status("Name:\tfb\nGid:\t1000\n"), None);
        assert_eq!(parse_uid_from_status(""), None);
        assert_eq!(parse_uid_from_status("Uid:\tnot-a-number\n"), None);
    }

    #[test]
    fn controllers_present_means_nothing_missing() {
        assert!(missing_controllers("cpu memory pids\n", REQUIRED_CONTROLLERS).is_empty());
        assert!(missing_controllers("cpu memory pids cpuset io", REQUIRED_CONTROLLERS).is_empty());
    }

    #[test]
    fn controllers_reports_only_required_missing() {
        let missing = missing_controllers("cpu pids", REQUIRED_CONTROLLERS);
        assert_eq!(missing, vec!["memory"]);
    }

    #[test]
    fn controllers_empty_reports_all_missing() {
        assert_eq!(
            missing_controllers("", REQUIRED_CONTROLLERS),
            vec!["cpu", "memory", "pids"]
        );
    }

    #[test]
    fn controllers_extra_names_are_not_required() {
        assert!(parse_controllers("io cpuset freezer").contains(&"io"));
    }

    #[test]
    fn cpuset_presence_is_detectable() {
        assert!(parse_controllers("cpu memory pids cpuset").contains(&"cpuset"));
        assert!(!parse_controllers("cpu memory pids").contains(&"cpuset"));
    }

    #[test]
    fn git_version_modern_parses() {
        assert_eq!(parse_git_version("git version 2.43.0"), Some((2, 43)));
        assert_eq!(
            parse_git_version("git version 2.43.0.windows.1"),
            Some((2, 43))
        );
    }

    #[test]
    fn git_version_exactly_minimum() {
        assert_eq!(parse_git_version("git version 2.5"), Some((2, 5)));
        assert!((2, 5) >= (2, GIT_WORKTREE_MINOR));
    }

    #[test]
    fn git_version_old_and_garbage() {
        assert_eq!(parse_git_version("git version 1.8.3"), Some((1, 8)));
        assert!(parse_git_version("1.8.3") < (2, GIT_WORKTREE_MINOR).into());
        assert_eq!(parse_git_version("not a version"), None);
        assert_eq!(parse_git_version(""), None);
        assert_eq!(parse_git_version("git version"), None);
    }

    #[test]
    fn df_line_parses_free_kib() {
        let line = "/dev/sda1 1000000 600000 400000 40% /home/gabe";
        assert_eq!(parse_df_available_kb(line), Some(400000));
    }

    #[test]
    fn df_header_and_bad_lines_are_skipped() {
        assert_eq!(
            parse_df_available_kb("Filesystem 1024-blocks Used Available Capacity Mounted on"),
            None
        );
        assert_eq!(parse_df_available_kb("/dev/sda1 1 2 nope 40% /"), None);
        assert_eq!(parse_df_available_kb(""), None);
        assert_eq!(parse_df_available_kb("/dev/sda1 1 2 3"), None);
    }

    #[test]
    fn path_splits_into_dirs() {
        assert_eq!(split_path("/usr/bin:/bin:"), vec!["/usr/bin", "/bin", ""]);
        assert_eq!(split_path(""), vec![""]);
    }

    #[test]
    fn gpu_line_parses() {
        assert_eq!(
            parse_gpu_line("NVIDIA GeForce RTX 4090, 550.54.07"),
            Some((
                "NVIDIA GeForce RTX 4090".to_string(),
                "550.54.07".to_string()
            ))
        );
        assert_eq!(parse_gpu_line(", 550.54"), None);
        assert_eq!(parse_gpu_line("NVIDIA GeForce RTX 4090"), None);
        assert_eq!(parse_gpu_line(""), None);
    }

    #[test]
    fn human_bytes_formats() {
        assert_eq!(human_bytes(0), "0.0 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(10 * 1024 * 1024 * 1024), "10.0 GiB");
    }

    #[test]
    fn status_serializes_to_spec_spelling() {
        assert_eq!(serde_json::to_string(&Status::Ok).unwrap(), "\"Ok\"");
        assert_eq!(serde_json::to_string(&Status::Warn).unwrap(), "\"Warn\"");
        assert_eq!(serde_json::to_string(&Status::Fail).unwrap(), "\"Fail\"");
    }

    #[test]
    fn check_serializes_with_all_fields() {
        let check = Check::new(
            "sample",
            Status::Fail,
            "something broke",
            Some("fix it".to_string()),
        );
        let value = serde_json::to_value(&check).unwrap();
        assert_eq!(value["name"], "sample");
        assert_eq!(value["status"], "Fail");
        assert_eq!(value["message"], "something broke");
        assert_eq!(value["fix"], "fix it");
    }

    // ---- repository-check fixtures -------------------------------------------------

    use std::sync::atomic::{AtomicU64, Ordering};

    static FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// A fresh, empty directory under the system temp dir, unique to this call. Never reused
    /// across tests, so parallel `cargo test` threads cannot interfere with each other even
    /// though they share one process id.
    fn fixture_dir(tag: &str) -> PathBuf {
        let n = FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("fb-doctor-test-{tag}-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).expect("create fixture dir");
        dir
    }

    fn write_fixture(dir: &Path, rel: &str, contents: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create fixture parent dir");
        }
        fs::write(path, contents).expect("write fixture file");
    }

    // ---- check_rustfmt ---------------------------------------------------------------

    #[test]
    fn rustfmt_zero_rs_files_is_ok_without_running_cargo() {
        // No Cargo.toml either: if this reached `cargo fmt` it would fail to find a
        // manifest and this would come back Warn, not Ok. Reaching Ok here proves the
        // zero-files shortcut fires before cargo is ever invoked.
        let dir = fixture_dir("rustfmt-zero");
        let check = check_rustfmt(&dir);
        assert_eq!(check.status, Status::Ok);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rustfmt_clean_tree_is_ok() {
        let dir = fixture_dir("rustfmt-clean");
        write_fixture(
            &dir,
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        write_fixture(
            &dir,
            "src/main.rs",
            "fn main() {\n    println!(\"hi\");\n}\n",
        );
        let check = check_rustfmt(&dir);
        assert_eq!(check.status, Status::Ok, "message was: {}", check.message);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rustfmt_dirty_tree_is_warn_with_file_count_and_fix() {
        let dir = fixture_dir("rustfmt-dirty-one");
        write_fixture(
            &dir,
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        write_fixture(
            &dir,
            "src/main.rs",
            "fn main(){\nlet x=1;\nprintln!(\"{}\",x);\n}\n",
        );
        let check = check_rustfmt(&dir);
        assert_eq!(check.status, Status::Warn);
        assert!(
            check.message.contains("1 file") && !check.message.contains("1 files"),
            "message was: {}",
            check.message
        );
        assert_eq!(check.fix, Some("cargo fmt --all".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rustfmt_counts_files_not_hunks() {
        // Two files, each with several independent formatting violations far apart in the
        // file, so a hunk-counting implementation would report more than 2.
        let dir = fixture_dir("rustfmt-dirty-two");
        write_fixture(
            &dir,
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        // `mod other;` is required: rustfmt walks the module tree from the crate root, so a
        // loose src/other.rs with no `mod` declaration pulling it in would be invisible to
        // `cargo fmt --all` and this test would (incorrectly) see only 1 dirty file.
        let messy = "fn a(){\nlet x=1;\nprintln!(\"{}\",x);\n}\n\nfn b(){\nlet y=2;\nprintln!(\"{}\",y);\n}\n\nfn c(){\nlet z=3;\nprintln!(\"{}\",z);\n}\n";
        write_fixture(&dir, "src/main.rs", &format!("mod other;\n\n{messy}"));
        write_fixture(&dir, "src/other.rs", messy);
        let check = check_rustfmt(&dir);
        assert_eq!(check.status, Status::Warn);
        assert!(
            check.message.contains("2 files"),
            "message was: {}",
            check.message
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rustfmt_missing_cargo_toml_is_warn_never_ok() {
        // Has a .rs file (so the zero-files shortcut does not fire) but no Cargo.toml, so
        // `cargo fmt` cannot even determine what to format.
        let dir = fixture_dir("rustfmt-no-manifest");
        write_fixture(&dir, "src/main.rs", "fn main() {}\n");
        let check = check_rustfmt(&dir);
        assert_eq!(check.status, Status::Warn);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rustfmt_nonexistent_repo_is_warn_never_ok() {
        let dir = std::env::temp_dir().join("fb-doctor-test-rustfmt-does-not-exist");
        let _ = fs::remove_dir_all(&dir);
        let check = check_rustfmt(&dir);
        assert_eq!(check.status, Status::Warn);
    }

    #[test]
    fn count_noun_pluralizes_correctly() {
        assert_eq!(count_noun(0, "file"), "0 files");
        assert_eq!(count_noun(1, "file"), "1 file");
        assert_eq!(count_noun(2, "file"), "2 files");
    }

    #[test]
    fn count_rs_files_skips_target_and_dotdirs() {
        let dir = fixture_dir("count-rs");
        write_fixture(&dir, "src/main.rs", "");
        write_fixture(&dir, "src/nested/lib.rs", "");
        write_fixture(&dir, "target/debug/build/gen.rs", "");
        write_fixture(&dir, ".git/hooks/gen.rs", "");
        write_fixture(&dir, "README.md", "");
        assert_eq!(count_rs_files(&dir), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    // ---- check_launcher_coverage ------------------------------------------------------

    const DISPATCH_FIXTURE: &str = r#"
case "$SRC" in
    codex-luna)      run_confined codex "$P" ;;
    claude-sonnet)   run_confined claude "$P" ;;
    ifm-*)           run_confined opencode -m "$MODEL" "$P" ;;
    or-*)            run_confined ori opencode run -m "$MODEL" "$P" ;;
    *) echo "unknown source $SRC" \
            "no launcher"
       exit 127 ;;
esac
"#;

    fn sources_fixture(extra: &str) -> String {
        format!(
            "[source.claude-sonnet]\nstatus = \"verified\"\n\n\
             [source.codex-luna]\nstatus = \"verified\"\n\n\
             [source.ifm-k2-horizon]\nstatus = \"verified\"\n\n\
             [source.or-hy3]\nstatus = \"verified\"\n\n{extra}"
        )
    }

    #[test]
    fn coverage_all_covered_is_ok() {
        let dir = fixture_dir("coverage-ok");
        write_fixture(&dir, "sources.toml", &sources_fixture(""));
        write_fixture(&dir, "crates/fb/src/launch.rs", DISPATCH_FIXTURE);
        let check = check_launcher_coverage(&dir);
        assert_eq!(check.status, Status::Ok, "message was: {}", check.message);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn coverage_missing_arm_is_fail_and_names_it() {
        let dir = fixture_dir("coverage-missing");
        let extra = "[source.gemini-38-flash]\nstatus = \"verified\"\n";
        write_fixture(&dir, "sources.toml", &sources_fixture(extra));
        write_fixture(&dir, "crates/fb/src/launch.rs", DISPATCH_FIXTURE);
        let check = check_launcher_coverage(&dir);
        assert_eq!(check.status, Status::Fail);
        assert!(
            check.message.contains("gemini-38-flash"),
            "message was: {}",
            check.message
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn coverage_disabled_and_redundant_arms_are_not_required() {
        let dir = fixture_dir("coverage-excluded");
        let extra = "[source.parked-thing]\nstatus = \"disabled\"\n\n\
                     [source.paid-duplicate]\nstatus = \"verified\"\nredundant_with = \"codex-luna\"\n";
        write_fixture(&dir, "sources.toml", &sources_fixture(extra));
        write_fixture(&dir, "crates/fb/src/launch.rs", DISPATCH_FIXTURE);
        let check = check_launcher_coverage(&dir);
        assert_eq!(check.status, Status::Ok, "message was: {}", check.message);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn coverage_zero_arms_is_ok() {
        let dir = fixture_dir("coverage-zero");
        write_fixture(&dir, "sources.toml", "");
        write_fixture(&dir, "crates/fb/src/launch.rs", DISPATCH_FIXTURE);
        let check = check_launcher_coverage(&dir);
        assert_eq!(check.status, Status::Ok);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn coverage_missing_sources_toml_is_warn_and_names_it() {
        let dir = fixture_dir("coverage-no-sources");
        write_fixture(&dir, "crates/fb/src/launch.rs", DISPATCH_FIXTURE);
        let check = check_launcher_coverage(&dir);
        assert_eq!(check.status, Status::Warn);
        assert!(
            check.message.contains("sources.toml"),
            "message was: {}",
            check.message
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn coverage_missing_dispatch_script_is_warn_and_names_it() {
        let dir = fixture_dir("coverage-no-dispatch");
        write_fixture(&dir, "sources.toml", &sources_fixture(""));
        let check = check_launcher_coverage(&dir);
        assert_eq!(check.status, Status::Warn);
        assert!(
            check.message.contains("launch.rs"),
            "message was: {}",
            check.message
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn classify_patterns_excludes_bare_wildcard() {
        let patterns = vec![
            "claude-sonnet".to_string(),
            "ifm-*".to_string(),
            "*".to_string(),
        ];
        let rules = classify_patterns(&patterns);
        assert_eq!(rules.exact, vec!["claude-sonnet".to_string()]);
        assert_eq!(rules.prefix, vec!["ifm-".to_string()]);
        // The bare "*" must not have become an empty-string prefix: that would match
        // everything, silently granting coverage to any arm at all.
        assert!(!is_covered("literally-anything", &rules));
    }

    #[test]
    fn is_covered_matches_exact_and_prefix() {
        let rules = classify_patterns(&["claude-sonnet".to_string(), "or-*".to_string()]);
        assert!(is_covered("claude-sonnet", &rules));
        assert!(is_covered("or-hy3", &rules));
        assert!(!is_covered("gemini-38-flash", &rules));
        assert!(!is_covered("claude-sonnet-2", &rules)); // exact match, not a prefix match
    }

    #[test]
    fn uncovered_arms_empty_input_yields_empty_output() {
        let rules = classify_patterns(&["or-*".to_string()]);
        assert_eq!(uncovered_arms(&[], &rules), Vec::<String>::new());
    }

    #[test]
    fn uncovered_arms_is_sorted() {
        let rules = classify_patterns(&["or-*".to_string()]);
        let dispatchable = vec!["zzz-arm".to_string(), "aaa-arm".to_string()];
        assert_eq!(
            uncovered_arms(&dispatchable, &rules),
            vec!["aaa-arm".to_string(), "zzz-arm".to_string()]
        );
    }

    #[test]
    fn dispatch_case_patterns_extracts_and_splits_on_pipe() {
        let script =
            "case \"$SRC\" in\n  a|b)   run a ;;\n  c-*)   run c ;;\n  *)     fail ;;\nesac\n";
        assert_eq!(
            dispatch_case_patterns(script),
            vec![
                "a".to_string(),
                "b".to_string(),
                "c-*".to_string(),
                "*".to_string()
            ]
        );
    }

    #[test]
    fn dispatch_case_patterns_empty_when_no_block_found() {
        assert_eq!(
            dispatch_case_patterns("#!/bin/bash\necho hi\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn dispatchable_arms_excludes_disabled_and_redundant() {
        let doc: toml::Value = toml::from_str(
            "[source.a]\nstatus = \"verified\"\n\n\
             [source.b]\nstatus = \"disabled\"\n\n\
             [source.c]\nstatus = \"verified\"\nredundant_with = \"a\"\n\n\
             [source.d]\n",
        )
        .unwrap();
        assert_eq!(
            dispatchable_arms(&doc),
            vec!["a".to_string(), "d".to_string()]
        );
    }

    #[test]
    fn dispatchable_arms_empty_table_is_empty() {
        let doc: toml::Value = toml::from_str("").unwrap();
        assert_eq!(dispatchable_arms(&doc), Vec::<String>::new());
    }

    // ---- check_spec_targets ------------------------------------------------------------

    #[test]
    fn spec_targets_ok_when_all_declare_and_none_stale() {
        let dir = fixture_dir("spec-ok");
        write_fixture(&dir, "crates/fb/src/existing.rs", "// already here\n");
        write_fixture(
            &dir,
            ".fb/prompts/a.md",
            "<!-- fb:modifies crates/fb/src/existing.rs -->\n# Task\n",
        );
        write_fixture(
            &dir,
            ".fb/prompts/b.md",
            "<!-- fb:creates crates/fb/src/new_thing.rs -->\n# Task\n",
        );
        let check = check_spec_targets(&dir);
        assert_eq!(check.status, Status::Ok, "message was: {}", check.message);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn spec_targets_stale_create_is_warn_naming_spec_and_path() {
        let dir = fixture_dir("spec-stale");
        write_fixture(&dir, "crates/fb/src/already_here.rs", "// oops\n");
        write_fixture(
            &dir,
            ".fb/prompts/stale-task.md",
            "<!-- fb:creates crates/fb/src/already_here.rs -->\n# Task\n",
        );
        let check = check_spec_targets(&dir);
        assert_eq!(check.status, Status::Warn);
        assert!(
            check.message.contains("stale-task.md"),
            "message was: {}",
            check.message
        );
        assert!(
            check.message.contains("crates/fb/src/already_here.rs"),
            "message was: {}",
            check.message
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn spec_targets_no_target_is_warn_named_separately_from_stale() {
        let dir = fixture_dir("spec-mixed");
        write_fixture(&dir, "crates/fb/src/already_here.rs", "// oops\n");
        write_fixture(
            &dir,
            ".fb/prompts/stale-task.md",
            "<!-- fb:creates crates/fb/src/already_here.rs -->\n# Task\n",
        );
        write_fixture(
            &dir,
            ".fb/prompts/no-target-task.md",
            "# Task with no markers\n",
        );
        let check = check_spec_targets(&dir);
        assert_eq!(check.status, Status::Warn);
        // Both faults must be visible, and distinguishably so: a reader must be able to tell
        // "already exists" apart from "declared nothing" rather than see one merged list.
        assert!(check.message.contains("stale-task.md"));
        assert!(check.message.contains("no-target-task.md"));
        assert!(check.message.contains("already exists"));
        assert!(check.message.contains("no fb:creates/fb:modifies target"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn spec_targets_empty_dir_is_ok() {
        let dir = fixture_dir("spec-empty");
        fs::create_dir_all(dir.join(".fb/prompts")).unwrap();
        let check = check_spec_targets(&dir);
        assert_eq!(check.status, Status::Ok);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn spec_targets_ignores_non_markdown_files() {
        let dir = fixture_dir("spec-non-md");
        write_fixture(&dir, ".fb/prompts/README.txt", "not a spec\n");
        let check = check_spec_targets(&dir);
        assert_eq!(check.status, Status::Ok, "message was: {}", check.message);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn spec_targets_reports_only_the_stale_target_among_several() {
        let dir = fixture_dir("spec-partial-stale");
        write_fixture(&dir, "crates/fb/src/exists.rs", "// here\n");
        write_fixture(
            &dir,
            ".fb/prompts/two-targets.md",
            "<!-- fb:creates crates/fb/src/exists.rs -->\n<!-- fb:creates crates/fb/src/not_yet.rs -->\n",
        );
        let check = check_spec_targets(&dir);
        assert_eq!(check.status, Status::Warn);
        assert!(check.message.contains("exists.rs"));
        assert!(!check.message.contains("not_yet.rs"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_spec_targets_preserves_order_and_ignores_other_lines() {
        let text = "<!-- fb:reads other.sh -->\n\
                     <!-- fb:creates a.rs -->\n\
                     some prose\n\
                     <!-- fb:modifies b.rs -->\n\
                     <!-- fb:creates c.rs -->\n";
        let targets = parse_spec_targets(text);
        assert_eq!(
            targets.creates,
            vec!["a.rs".to_string(), "c.rs".to_string()]
        );
        assert_eq!(targets.modifies, vec!["b.rs".to_string()]);
    }

    #[test]
    fn parse_spec_targets_no_markers_is_two_empty_lists() {
        let targets = parse_spec_targets("# just a heading\nsome text\n");
        assert!(targets.creates.is_empty());
        assert!(targets.modifies.is_empty());
    }

    #[test]
    fn spec_md_files_sorted_and_filters_extension() {
        let dir = fixture_dir("spec-md-files");
        write_fixture(&dir, "z.md", "");
        write_fixture(&dir, "a.md", "");
        write_fixture(&dir, "notes.txt", "");
        let files: Vec<String> = spec_md_files(&dir)
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
            .collect();
        assert_eq!(files, vec!["a.md".to_string(), "z.md".to_string()]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn spec_md_files_missing_dir_is_empty() {
        let dir = std::env::temp_dir().join("fb-doctor-test-spec-md-does-not-exist");
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(spec_md_files(&dir), Vec::<PathBuf>::new());
    }

    // ---- orphaned modules -------------------------------------------------------------

    #[test]
    fn orphan_decision_uses_core_and_reports_all_orphans() {
        let crates = vec![CrateSources {
            name: "fixture".to_string(),
            stems: vec![
                "lib".to_string(),
                "declared".to_string(),
                "missing".to_string(),
                "also_missing".to_string(),
            ],
            lib_rs: Some("mod declared;\n".to_string()),
            main_rs: None,
        }];
        let check = decide_orphaned_modules(&crates);
        assert_eq!(check.name, "orphaned modules");
        assert_eq!(check.status, Status::Fail);
        assert!(check.message.contains("fixture:missing"));
        assert!(check.message.contains("fixture:also_missing"));
        assert!(check.fix.is_some());
    }

    #[test]
    fn orphan_scan_handles_binary_roots_and_ignores_nested_non_rs_files() {
        let dir = fixture_dir("orphan-scan");
        write_fixture(&dir, "crates/bin/src/main.rs", "mod declared;\n");
        write_fixture(&dir, "crates/bin/src/declared.rs", "");
        write_fixture(&dir, "crates/bin/src/orphan.rs", "");
        write_fixture(&dir, "crates/bin/src/notes.txt", "");
        write_fixture(&dir, "crates/bin/src/nested/ignored.rs", "");
        write_fixture(&dir, "crates/no-src/README.md", "");
        let check = check_orphaned_modules(&dir);
        assert_eq!(check.status, Status::Fail);
        assert!(check.message.contains("bin:orphan"));
        assert!(!check.message.contains("notes"));
        assert!(!check.message.contains("ignored"));
        assert!(!check.message.contains("no-src"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn orphan_check_with_no_crates_says_nothing_was_examined() {
        let dir = fixture_dir("orphan-zero");
        let check = check_orphaned_modules(&dir);
        assert_eq!(check.status, Status::Ok);
        assert!(!check.message.is_empty());
        assert!(check.message.contains("no crates were examined"));
        assert_eq!(check.fix, None);
        let _ = fs::remove_dir_all(&dir);
    }

    // ---- unreachable modules ----------------------------------------------------------

    /// Build a scanned crate the way `scan_crate_sources` would, without the filesystem.
    fn scanned_binary(name: &str, main: &str, modules: &[(&str, &str)]) -> ScannedCrate {
        let mut stems: Vec<String> = modules.iter().map(|(s, _)| (*s).to_string()).collect();
        stems.insert(0, "main".to_string());
        ScannedCrate {
            sources: CrateSources {
                name: name.to_string(),
                stems,
                lib_rs: None,
                main_rs: Some(main.to_string()),
            },
            bodies: modules
                .iter()
                .map(|(s, body)| ((*s).to_string(), (*body).to_string()))
                .collect(),
        }
    }

    #[test]
    fn unreachable_transitive_reach_stays_ok() {
        // main names `a::`, `a.rs` names `b::`: both reachable, nothing to report.
        let krate = scanned_binary(
            "fixture",
            "fn main() { a::run(); }",
            &[("a", "pub fn run() { b::go(); }"), ("b", "pub fn go() {}")],
        );
        let check = decide_unreachable_modules(&[krate]);
        assert_eq!(check.name, "unreachable modules");
        assert_eq!(check.status, Status::Ok);
        assert!(!check.message.is_empty());
        assert_eq!(check.fix, None);
    }

    #[test]
    fn unreachable_module_named_by_nothing_is_warn_and_named() {
        // Clause 2, plus clause 7 pinned against the orphan check's Fail: identical input
        // goes through both decides, so the two severities are compared directly.
        let krate = scanned_binary("fixture", "fn main() {}", &[("dead_mod", "")]);
        let check = decide_unreachable_modules(std::slice::from_ref(&krate));
        assert_eq!(check.status, Status::Warn);
        assert!(check.message.contains("dead_mod"));
        assert!(check.fix.is_some());
        let orphan = decide_orphaned_modules(&orphan_sources(&[krate]));
        assert_eq!(orphan.status, Status::Fail);
        assert_ne!(check.status, orphan.status);
    }

    #[test]
    fn unreachable_self_naming_is_not_reachable() {
        // foo.rs mentioning `foo::` must not keep itself alive.
        let krate = scanned_binary(
            "fixture",
            "fn main() {}",
            &[("foo", "fn touch() { foo::nothing(); }")],
        );
        let check = decide_unreachable_modules(&[krate]);
        assert_eq!(check.status, Status::Warn);
        assert!(check.message.contains("foo"));
    }

    #[test]
    fn unreachable_cycle_of_two_is_both_reported() {
        // a names b, b names a, main names neither: reachable from itself, and from nothing.
        let krate = scanned_binary(
            "fixture",
            "fn main() {}",
            &[("a", "use super::b::go;"), ("b", "use super::a::go;")],
        );
        // Neither `a` nor `b` appears as a path prefix anywhere from main, so both die.
        assert_eq!(
            reachable_stems("fn main() {}", &krate.bodies),
            Vec::<String>::new()
        );
        let check = decide_unreachable_modules(&[krate]);
        assert_eq!(check.status, Status::Warn);
        assert!(check.message.contains("a"));
        assert!(check.message.contains("b"));
    }

    #[test]
    fn unreachable_cycle_joined_from_main_is_reachable() {
        // Same cycle, but main names one member: transitive closure must flood both.
        let reachable = reachable_stems(
            "fn main() { a::go(); }",
            &[
                ("a".to_string(), "b::go();".to_string()),
                ("b".to_string(), "a::go();".to_string()),
            ],
        );
        assert!(reachable.contains(&"a".to_string()));
        assert!(reachable.contains(&"b".to_string()));
    }

    #[test]
    fn unreachable_library_pub_mod_is_never_reported() {
        let krate = ScannedCrate {
            sources: CrateSources {
                name: "libcrate".to_string(),
                stems: vec!["lib".to_string(), "public".to_string()],
                lib_rs: Some("pub mod public;".to_string()),
                main_rs: None,
            },
            bodies: vec![("public".to_string(), "pub fn unused() {}".to_string())],
        };
        let check = decide_unreachable_modules(&[krate]);
        assert_eq!(check.status, Status::Ok);
        assert!(!check.message.contains("public"));
    }

    #[test]
    fn unreachable_roots_are_never_reported() {
        // A binary crate with ONLY main.rs: nothing to report, Ok with a real message.
        let krate = ScannedCrate {
            sources: CrateSources {
                name: "tiny".to_string(),
                stems: vec!["main".to_string()],
                lib_rs: None,
                main_rs: Some("fn main() {}".to_string()),
            },
            bodies: Vec::new(),
        };
        let check = decide_unreachable_modules(&[krate]);
        assert_eq!(check.status, Status::Ok);
        assert!(!check.message.is_empty());
    }

    #[test]
    fn unreachable_zero_crates_is_ok_not_empty() {
        let check = decide_unreachable_modules(&[]);
        assert_eq!(check.status, Status::Ok);
        assert!(!check.message.is_empty());
        assert_eq!(check.fix, None);
    }

    #[test]
    fn unreachable_message_names_every_one_found() {
        let krate = scanned_binary(
            "fixture",
            "fn main() { live::run(); }",
            &[
                ("live", "pub fn run() { helper::go(); }"),
                ("helper", "pub fn go() {}"),
                ("first_dead", ""),
                ("second_dead", ""),
            ],
        );
        let check = decide_unreachable_modules(&[krate]);
        assert_eq!(check.status, Status::Warn);
        assert!(check.message.contains("first_dead"));
        assert!(check.message.contains("second_dead"));
        assert!(!check.message.contains("live"));
        assert!(!check.message.contains("helper"));
    }

    #[test]
    fn unreachable_word_boundary_and_path_marker_are_required() {
        // `adead::` does not make `dead` reachable; `dead` without `::` does not either.
        assert!(!names_as_path("adead::go()", "dead"));
        assert!(!names_as_path("let dead = 1;", "dead"));
        assert!(names_as_path("dead::go()", "dead"));
        assert!(names_as_path("(dead::go)", "dead"));
    }

    #[test]
    fn unreachable_scan_reuses_the_orphan_walk() {
        // One scan feeds both checks: same fixture directory, same ScannedCrate list.
        let dir = fixture_dir("unreachable-scan");
        write_fixture(
            &dir,
            "crates/bin/src/main.rs",
            "mod used;\nmod unused;\nfn main() { used::go(); }\n",
        );
        write_fixture(&dir, "crates/bin/src/used.rs", "pub fn go() {}\n");
        write_fixture(&dir, "crates/bin/src/unused.rs", "pub fn never() {}\n");
        let scanned = scan_crate_sources(&dir.join("crates"));
        let orphan = decide_orphaned_modules(&orphan_sources(&scanned));
        let reach = decide_unreachable_modules(&scanned);
        assert_eq!(orphan.status, Status::Ok);
        assert_eq!(reach.status, Status::Warn);
        assert!(reach.message.contains("bin:unused"));
        assert!(!reach.message.contains("bin:used"));
        // A second scan of the same tree sees the same answer.
        let recomputed = decide_unreachable_modules(&scan_crate_sources(&dir.join("crates")));
        assert_eq!(recomputed.status, reach.status);
        let _ = fs::remove_dir_all(&dir);
    }
}
