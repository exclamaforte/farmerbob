//! Adapter contract for invoking agent CLIs declaratively.
//!
//! This module defines the pure-logic contract between farmerbob and the many
//! agent backends it supports. No process spawning, filesystem access, or
//! clock dependencies — everything is passed in, making it fully testable.

use serde::{Deserialize, Serialize};

/// Unique identifier for an agent arm.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArmId(pub String);

/// Declarative specification for how an agent arm is invoked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdapterSpec {
    /// The arm this spec belongs to.
    pub arm: ArmId,
    /// Program to run, e.g. "codex".
    pub program: String,
    /// Argv template. The literal tokens `{prompt}` and `{worktree}` are substituted.
    pub args: Vec<String>,
    /// Whether the program runs in the working directory it is given. When false the
    /// worktree must be passed explicitly via `--add-dir`.
    pub cwd_honored: bool,
    /// Environment to set, as key/value pairs.
    pub env: Vec<(String, String)>,
    /// Substrings that identify a usage-limit refusal in the arm's output.
    pub limit_markers: Vec<String>,
    /// Substrings that identify an auth failure.
    pub auth_markers: Vec<String>,
}

impl AdapterSpec {
    /// Build the argv. `{prompt}` and `{worktree}` are replaced wherever they appear as a
    /// whole token. When `cwd_honored` is false, push `--add-dir <worktree>` as the last
    /// two arguments so the arm cannot escape its worktree.
    pub fn argv(&self, prompt: &str, worktree: &str) -> Vec<String> {
        let mut argv: Vec<String> = self
            .args
            .iter()
            .map(|arg| {
                if arg == "{prompt}" {
                    prompt.to_string()
                } else if arg == "{worktree}" {
                    worktree.to_string()
                } else {
                    arg.clone()
                }
            })
            .collect();

        if !self.cwd_honored {
            argv.push("--add-dir".to_string());
            argv.push(worktree.to_string());
        }

        argv
    }

    /// Classify an exit. `status` is the raw wait status code as the shell reports it, so
    /// 128+N means signal N; 143 is SIGTERM and 137 is SIGKILL. `output` is the arm's
    /// combined stdout and stderr.
    ///
    /// Precedence, highest first: usage limit, auth failure, signal, non-zero code,
    /// otherwise completed. A limit marker in the output wins even when the exit code is 0,
    /// because several backends report a refusal and then exit cleanly.
    pub fn classify(&self, status: i32, output: &str) -> ExitKind {
        let output_lower = output.to_lowercase();

        // Check usage limit markers (highest precedence)
        for marker in &self.limit_markers {
            if output_lower.contains(&marker.to_lowercase()) {
                return ExitKind::UsageLimit {
                    marker: marker.clone(),
                };
            }
        }

        // Check auth failure markers
        for marker in &self.auth_markers {
            if output_lower.contains(&marker.to_lowercase()) {
                return ExitKind::AuthFailure {
                    marker: marker.clone(),
                };
            }
        }

        // Check for signal termination (128 + signal number)
        if (129..=192).contains(&status) {
            return ExitKind::Signalled {
                signal: status - 128,
            };
        }

        // Non-zero exit code
        if status != 0 {
            return ExitKind::Crashed { code: status };
        }

        ExitKind::Completed
    }
}

/// Why a run ended. The distinctions matter: only `Completed` says anything about the arm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExitKind {
    Completed,
    UsageLimit {
        marker: String,
    },
    AuthFailure {
        marker: String,
    },
    /// Terminated by a signal. `signal` is the raw signal number.
    Signalled {
        signal: i32,
    },
    Crashed {
        code: i32,
    },
}

/// Only `Completed` may update an arm's success statistics. Everything else is an
/// infrastructure outcome and must be excluded.
pub fn counts_for_posterior(kind: &ExitKind) -> bool {
    matches!(kind, ExitKind::Completed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> AdapterSpec {
        AdapterSpec {
            arm: ArmId("test-arm".to_string()),
            program: "test-agent".to_string(),
            args: vec![
                "--prompt".to_string(),
                "{prompt}".to_string(),
                "--dir".to_string(),
                "{worktree}".to_string(),
            ],
            cwd_honored: true,
            env: vec![],
            limit_markers: vec!["rate limit exceeded".to_string(), "usage limit".to_string()],
            auth_markers: vec![
                "authentication failed".to_string(),
                "unauthorized".to_string(),
            ],
        }
    }

    #[test]
    fn whole_token_substitution() {
        let s = spec();
        let argv = s.argv("hello world", "/tmp/work");
        assert_eq!(argv[1], "hello world");
        assert_eq!(argv[3], "/tmp/work");
    }

    #[test]
    fn embedded_token_not_substituted() {
        let mut s = spec();
        s.args = vec!["--arg=prefix{prompt}suffix".to_string()];
        let argv = s.argv("hello", "/tmp/work");
        assert_eq!(argv[0], "--arg=prefix{prompt}suffix");
    }

    #[test]
    fn add_dir_appended_once_when_cwd_not_honored() {
        let mut s = spec();
        s.cwd_honored = false;
        let argv = s.argv("prompt", "/tmp/work");
        let add_dir_count = argv.iter().filter(|a| *a == "--add-dir").count();
        assert_eq!(add_dir_count, 1);
        assert_eq!(argv[argv.len() - 2], "--add-dir");
        assert_eq!(argv[argv.len() - 1], "/tmp/work");
    }

    #[test]
    fn limit_marker_beats_exit_code_zero() {
        let s = spec();
        let kind = s.classify(0, "Operation failed: rate limit exceeded");
        assert!(matches!(kind, ExitKind::UsageLimit { .. }));
    }

    #[test]
    fn signal_143_classified_as_signalled_15() {
        let s = spec();
        let kind = s.classify(143, "");
        assert_eq!(kind, ExitKind::Signalled { signal: 15 });
    }

    #[test]
    fn counts_for_posterior_only_completed() {
        assert!(counts_for_posterior(&ExitKind::Completed));
        assert!(!counts_for_posterior(&ExitKind::UsageLimit {
            marker: "x".into()
        }));
        assert!(!counts_for_posterior(&ExitKind::AuthFailure {
            marker: "x".into()
        }));
        assert!(!counts_for_posterior(&ExitKind::Signalled { signal: 15 }));
        assert!(!counts_for_posterior(&ExitKind::Crashed { code: 1 }));
    }
}
