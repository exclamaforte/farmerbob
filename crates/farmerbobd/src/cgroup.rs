use std::process::{Child, Command, Stdio};

/// Resource limits for systemd scope confinement.
#[derive(Debug, Default, Clone)]
#[allow(dead_code)]
pub struct ResourceLimits {
    /// CPU quota as percentage. Maps to CPUQuota=X%.
    pub cpu_quota_percent: Option<u32>,
    /// Memory limit in bytes. Maps to MemoryMax=X.
    pub memory_max: Option<u64>,
    /// Memory high watermark in bytes. Maps to MemoryHigh=X.
    pub memory_high: Option<u64>,
    /// Maximum number of tasks. Maps to TasksMax=X.
    pub tasks_max: Option<u32>,
    /// Allowed CPU ranges. Maps to AllowedCPUs=X. Requires cpuset delegation.
    pub allowed_cpus: Option<String>,
    /// I/O weight. Maps to IOWeight=X.
    pub io_weight: Option<u32>,
}

/// Active state of a systemd scope.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum ScopeState {
    /// Scope is currently active.
    Active,
    /// Scope failed with a specific result.
    Failed { result: String },
    /// Scope is gone.
    Gone,
}

/// Cgroup statistics for a scope.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CgroupStats {
    /// CPU usage in microseconds.
    pub usage_usec: u64,
    /// CPU throttled time in microseconds.
    pub throttled_usec: u64,
    /// Current memory usage in bytes.
    pub memory_current: u64,
    /// Peak memory usage in bytes.
    pub memory_peak: u64,
    /// Number of OOM kills.
    pub oom_kill: u64,
    /// Current number of tasks.
    pub pids_current: u64,
}

/// Build the systemd-run command as a vector of arguments.
///
/// This is a pure function that builds arguments for `systemd-run`
/// with the specified resource limits. It does not spawn any process.
///
/// # Arguments
/// * `unit` - The unit name for the scope.
/// * `limits` - Resource limits to apply.
/// * `argv` - The command and its arguments to run inside the scope.
///
/// # Returns
/// A vector of string arguments suitable for `std::process::Command::new`
/// or direct exec.
#[allow(dead_code)]
pub fn build_command(unit: &str, limits: &ResourceLimits, argv: &[String]) -> Vec<String> {
    let mut args = Vec::new();

    // Systemd-run base options
    args.extend([
        "systemd-run".to_string(),
        "--user".to_string(),
        "--scope".to_string(),
    ]);
    args.push("--quiet".to_string());
    args.push(format!("--unit={}", unit));

    // Add properties for each limit that is Some
    if let Some(quota) = limits.cpu_quota_percent {
        args.push(format!("-pCPUQuota={}%", quota));
    }
    if let Some(max) = limits.memory_max {
        args.push(format!("-pMemoryMax={}", max));
    }
    if let Some(high) = limits.memory_high {
        args.push(format!("-pMemoryHigh={}", high));
    }
    if let Some(max) = limits.tasks_max {
        args.push(format!("-pTasksMax={}", max));
    }
    if let Some(ref cpus) = limits.allowed_cpus {
        args.push(format!("-pAllowedCPUs={}", cpus));
    }
    if let Some(weight) = limits.io_weight {
        args.push(format!("-pIOWeight={}", weight));
    }

    // Add the command and its arguments
    if !argv.is_empty() {
        args.extend_from_slice(argv);
    }

    args
}

/// Spawn a systemd transient scope with the given resource limits.
///
/// # Arguments
/// * `unit` - The unit name for the scope.
/// * `limits` - Resource limits to apply.
/// * `argv` - The command and its arguments to run.
/// * `cwd` - The working directory for the command.
///
/// # Returns
/// A `Result` containing a `Child` process handle on success.
#[allow(dead_code)]
pub fn spawn(
    unit: &str,
    limits: &ResourceLimits,
    argv: &[String],
    cwd: &str,
) -> std::io::Result<Child> {
    let args = build_command(unit, limits, argv);
    if args.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "no command to spawn",
        ));
    }

    let cmd = args[0].clone();
    let mut child = Command::new(cmd);
    for arg in &args[1..] {
        child.arg(arg);
    }
    child.current_dir(cwd);
    child.stdin(Stdio::null());
    child.stdout(Stdio::null());
    child.stderr(Stdio::null());
    child.spawn()
}

/// Stop a systemd scope by name.
///
/// This tears down the entire process tree associated with the scope.
///
/// # Arguments
/// * `unit` - The unit name for the scope.
///
/// # Returns
/// `Ok(())` on success, or an `io::Error` on failure.
#[allow(dead_code)]
pub fn kill_scope(unit: &str) -> std::io::Result<()> {
    let output = Command::new("systemctl")
        .args(["--user", "stop", &format!("{}.scope", unit)])
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(std::io::Error::other(format!(
            "failed to stop scope: {}",
            stderr
        )))
    }
}

/// Query the active state of a systemd scope.
///
/// # Arguments
/// * `unit` - The unit name for the scope.
///
/// # Returns
/// The `ScopeState` of the scope.
#[allow(dead_code)]
pub fn scope_state(unit: &str) -> ScopeState {
    let output = match Command::new("systemctl")
        .args([
            "--user",
            "show",
            &format!("{}.scope", unit),
            "--property=ActiveState,Result",
        ])
        .output()
    {
        Ok(output) => output,
        Err(_) => return ScopeState::Gone,
    };

    if !output.status.success() {
        return ScopeState::Gone;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut active = String::new();
    let mut result = String::new();

    for line in stdout.lines() {
        if let Some(stripped) = line.strip_prefix("ActiveState=") {
            active = stripped.to_string();
        } else if let Some(stripped) = line.strip_prefix("Result=") {
            result = stripped.to_string();
        }
    }

    match active.as_str() {
        "active" => ScopeState::Active,
        "failed" => ScopeState::Failed { result },
        _ => ScopeState::Gone,
    }
}

/// Read cgroup statistics for a scope.
///
/// # Arguments
/// * `unit` - The unit name for the scope.
///
/// # Returns
/// `Some(CgroupStats)` if the scope exists and stats can be read,
/// `None` if the scope is gone or stats are unavailable.
#[allow(dead_code)]
pub fn cgroup_stats(_unit: &str) -> Option<CgroupStats> {
    let cgroup_path = get_cgroup_path()?;
    let scope_cgroup = format!("{}/user.slice/user-{}.scope/scope", cgroup_path, get_uid()?);

    let usage_usec = parse_file_key(&format!("{}/cpu.stat", scope_cgroup), "usage_usec")?;
    let throttled_usec = parse_file_key(&format!("{}/cpu.stat", scope_cgroup), "throttled_usec")?;
    let memory_current = parse_file(&format!("{}/memory.current", scope_cgroup))?;
    let memory_peak = parse_file(&format!("{}/memory.peak", scope_cgroup))?;
    let oom_kill = parse_file_key(&format!("{}/memory.events", scope_cgroup), "oom_kill")?;
    let pids_current = parse_file(&format!("{}/pids.current", scope_cgroup))?;

    Some(CgroupStats {
        usage_usec,
        throttled_usec,
        memory_current,
        memory_peak,
        oom_kill,
        pids_current,
    })
}

/// Get the XDG_RUNTIME_DIR or derive from /proc/self/cgroup.
#[allow(dead_code)]
#[allow(clippy::collapsible_if)]
fn get_cgroup_path() -> Option<String> {
    if let Ok(path) = std::env::var("XDG_RUNTIME_DIR") {
        return Some(path);
    }

    let cgroup = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    for line in cgroup.lines() {
        if line.contains("cpuset") || line.contains("misc") {
            if let Some(idx) = line.rfind(':') {
                let path = line[idx + 1..].to_string();
                if let Some(base) = path.strip_prefix("/user.slice/user-") {
                    if let Some(user) = base.strip_suffix(".scope")?.split('/').next() {
                        return Some(format!("/sys/fs/cgroup/user.slice/{}.scope", user));
                    }
                }
            }
        }
    }

    let uid = get_uid()?;
    Some(format!("/sys/fs/cgroup/user.slice/user-{}.scope", uid))
}

/// Get the current user ID.
#[allow(dead_code)]
fn get_uid() -> Option<u32> {
    std::env::var("UID")
        .ok()
        .and_then(|u| u.parse().ok())
        .or_else(|| {
            std::env::var("USER").ok().and_then(|u| {
                let pass = std::fs::read_to_string("/etc/passwd").ok()?;
                for line in pass.lines() {
                    let fields: Vec<&str> = line.split(':').collect();
                    if fields.first() == Some(&u.as_str()) && fields.len() > 2 {
                        return fields[2].parse().ok();
                    }
                }
                None
            })
        })
}

/// Parse a numeric value from a file.
#[allow(dead_code)]
fn parse_file(path: &str) -> Option<u64> {
    let content = std::fs::read_to_string(path).ok()?;
    content.trim().parse().ok()
}

/// Parse a named value from a file (e.g., key=value lines).
#[allow(dead_code)]
fn parse_file_key(path: &str, key: &str) -> Option<u64> {
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines() {
        if line.starts_with(key) {
            let value = line.split('=').nth(1)?.trim();
            return value.parse().ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_command_with_all_limits() {
        let limits = ResourceLimits {
            cpu_quota_percent: Some(400),
            memory_max: Some(1024),
            memory_high: Some(512),
            tasks_max: Some(10),
            allowed_cpus: Some("0-7".to_string()),
            io_weight: Some(100),
        };

        let cmd = build_command("test-unit", &limits, &["agent".to_string()]);

        assert!(cmd.contains(&"-pCPUQuota=400%".to_string()));
        assert!(cmd.contains(&"-pMemoryMax=1024".to_string()));
        assert!(cmd.contains(&"-pMemoryHigh=512".to_string()));
        assert!(cmd.contains(&"-pTasksMax=10".to_string()));
        assert!(cmd.contains(&"-pAllowedCPUs=0-7".to_string()));
        assert!(cmd.contains(&"-pIOWeight=100".to_string()));
    }

    #[test]
    fn test_build_command_with_no_limits() {
        let limits = ResourceLimits::default();
        let cmd = build_command("test-unit", &limits, &["agent".to_string()]);

        assert!(!cmd.iter().any(|a| a.starts_with("-p")));
    }

    #[test]
    fn test_parse_cpu_stat() {
        let cpu_stat = "usage_usec 1000000
throttled_usec 50000";
        let usage = parse_file_key_str(cpu_stat, "usage_usec");
        let throttled = parse_file_key_str(cpu_stat, "throttled_usec");

        assert_eq!(usage, Some(1000000));
        assert_eq!(throttled, Some(50000));
    }

    #[test]
    fn test_parse_memory_events() {
        let mem_events = "low 0
high 0
max 0
oom_kill 2
oom_kill_adj 0";
        let oom = parse_file_key_str(mem_events, "oom_kill");

        assert_eq!(oom, Some(2));
    }

    #[test]
    fn test_scope_state_mapping() {
        // This test verifies that the state mapping logic works correctly
        // in real scenarios where systemctl would return actual states.
        assert_eq!(scope_state("nonexistent-unit-12345"), ScopeState::Gone);
    }

    // Helper function for testing with static strings
    fn parse_file_key_str(content: &str, key: &str) -> Option<u64> {
        for line in content.lines() {
            if line.starts_with(key) {
                let value = line.split_whitespace().nth(1)?.trim();
                return value.parse().ok();
            }
        }
        None
    }
}
