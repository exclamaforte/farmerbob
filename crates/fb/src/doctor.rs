//! `fb doctor` — environment preflight checks.
//!
//! Verifies that the current machine can actually run farmerbob: cgroup v2
//! delegation for confining agents, systemd, an NVIDIA GPU for serialized
//! inference, git worktree support, disk headroom and the presence of the
//! agent CLIs farmerbob drives in parallel.

use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    vec![
        check_cgroup_v2(),
        check_delegated_controllers(),
        check_systemd(),
        check_nvidia(),
        check_git_worktree(),
        check_disk_headroom(),
        check_agent_clis(),
    ]
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
}
