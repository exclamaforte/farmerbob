//! `fb live` — counting live agents by inspecting `/proc`.
//!
//! Replaces `fb-live.sh`. Everything else in this harness counts agents by
//! listing systemd scopes instead, which has failed twice: scopes left in a
//! failed state after timeouts are counted as live indefinitely, and critique
//! agents running without a scope report as zero. Checking process working
//! directories under `/proc` provides the true ground truth.

#![allow(dead_code)]

use std::path::{Component, Path};

/// One process the caller found, with the working directory it reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    /// The process id, for the caller's own reporting.
    pub pid: u32,
    /// The executable's name, e.g. `opencode`.
    pub comm: String,
    /// The resolved working directory, or `None` when `/proc/<pid>/cwd`
    /// could not be read -- a process that exited, or one owned by another
    /// user. `None` is NOT "not an agent".
    pub cwd: Option<String>,
}

/// How many of these processes are agents working in a worktree.
///
/// `root` is the worktree root, e.g.
/// `/home/gabe/.local/share/farmerbob/worktrees`.
pub fn count_live(procs: &[Proc], root: &str) -> usize {
    if root.is_empty() {
        return 0;
    }
    let root_path = Path::new(root);
    procs
        .iter()
        .filter(|p| {
            let Some(cwd) = &p.cwd else {
                return false;
            };
            let cwd_path = Path::new(cwd);
            match cwd_path.strip_prefix(root_path) {
                Ok(rel) => rel.components().any(|c| matches!(c, Component::Normal(_))),
                Err(_) => false,
            }
        })
        .count()
}

/// The processes whose working directory could not be read, so a caller can
/// say how many it could not classify rather than silently undercounting.
pub fn unreadable(procs: &[Proc]) -> Vec<u32> {
    procs
        .iter()
        .filter_map(|p| if p.cwd.is_none() { Some(p.pid) } else { None })
        .collect()
}

/// `fb live`: prints the count, and the unreadable pids when there are any.
/// Exits 0 always -- a count of zero is an answer.
pub fn run(args: &[String]) -> i32 {
    let mut root = crate::paths::worktrees().to_string_lossy().to_string();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--root" && i + 1 < args.len() {
            root = args[i + 1].clone();
            i += 2;
        } else if !args[i].starts_with('-') {
            root = args[i].clone();
            i += 1;
        } else {
            i += 1;
        }
    }

    let procs = gather_procs();
    let live = count_live(&procs, &root);
    let unread = unreadable(&procs);

    println!("{live}");
    if !unread.is_empty() {
        let pids = unread
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        println!("unreadable: {pids}");
    }
    0
}

/// Gathers candidate agent processes from `/proc`.
fn gather_procs() -> Vec<Proc> {
    let mut procs = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return procs;
    };

    for entry in entries.flatten() {
        let Ok(file_name) = entry.file_name().into_string() else {
            continue;
        };
        let Ok(pid) = file_name.parse::<u32>() else {
            continue;
        };
        let comm_path = format!("/proc/{pid}/comm");
        let Ok(comm_raw) = std::fs::read_to_string(&comm_path) else {
            continue;
        };
        let comm = comm_raw.trim().to_string();
        if is_candidate_agent(&comm) {
            let cwd = std::fs::read_link(format!("/proc/{pid}/cwd"))
                .ok()
                .map(|p| p.to_string_lossy().into_owned());
            procs.push(Proc { pid, comm, cwd });
        }
    }
    procs.sort_by_key(|p| p.pid);
    procs
}

/// Returns true if the executable name matches candidate agent binaries.
fn is_candidate_agent(comm: &str) -> bool {
    matches!(comm, "opencode" | "zcode" | "agy" | "codex")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clause_1_cwd_directly_under_root_and_nested_levels_count() {
        let root = "/home/gabe/.local/share/farmerbob/worktrees";
        let p1 = Proc {
            pid: 101,
            comm: "opencode".to_string(),
            cwd: Some("/home/gabe/.local/share/farmerbob/worktrees/wt-1".to_string()),
        };
        let p2 = Proc {
            pid: 102,
            comm: "zcode".to_string(),
            cwd: Some("/home/gabe/.local/share/farmerbob/worktrees/wt-2/nested/level3".to_string()),
        };
        assert_eq!(count_live(&[p1, p2], root), 2);
    }

    #[test]
    fn clause_2_root_exact_does_not_count() {
        let root = "/home/gabe/.local/share/farmerbob/worktrees";
        let p_exact = Proc {
            pid: 201,
            comm: "agy".to_string(),
            cwd: Some("/home/gabe/.local/share/farmerbob/worktrees".to_string()),
        };
        let p_exact_slash = Proc {
            pid: 202,
            comm: "codex".to_string(),
            cwd: Some("/home/gabe/.local/share/farmerbob/worktrees/".to_string()),
        };

        // Neither counts against root without trailing slash
        assert_eq!(count_live(std::slice::from_ref(&p_exact), root), 0);
        assert_eq!(count_live(std::slice::from_ref(&p_exact_slash), root), 0);

        // Neither counts against root with trailing slash
        let root_slash = "/home/gabe/.local/share/farmerbob/worktrees/";
        assert_eq!(count_live(&[p_exact], root_slash), 0);
        assert_eq!(count_live(&[p_exact_slash], root_slash), 0);
    }

    #[test]
    fn clauses_3_and_4_unreadable_and_outside_root_tested_together() {
        let root = "/home/gabe/.local/share/farmerbob/worktrees";
        let live = Proc {
            pid: 301,
            comm: "agy".to_string(),
            cwd: Some("/home/gabe/.local/share/farmerbob/worktrees/active-wt".to_string()),
        };
        let unreadable_proc = Proc {
            pid: 302,
            comm: "opencode".to_string(),
            cwd: None,
        };
        let sibling_shared_prefix = Proc {
            pid: 303,
            comm: "zcode".to_string(),
            cwd: Some("/home/gabe/.local/share/farmerbob/worktrees-old/x".to_string()),
        };
        let unrelated = Proc {
            pid: 304,
            comm: "codex".to_string(),
            cwd: Some("/tmp/scratch/other".to_string()),
        };

        let procs = vec![live, unreadable_proc, sibling_shared_prefix, unrelated];

        // Only the genuine live agent counts
        assert_eq!(count_live(&procs, root), 1);

        // Only the None cwd process is reported as unreadable; sibling and unrelated are not
        assert_eq!(unreadable(&procs), vec![302]);
    }

    #[test]
    fn clause_5_counts_processes_not_worktrees() {
        let root = "/home/gabe/.local/share/farmerbob/worktrees";
        let p1 = Proc {
            pid: 501,
            comm: "agy".to_string(),
            cwd: Some("/home/gabe/.local/share/farmerbob/worktrees/wt-shared".to_string()),
        };
        let p2 = Proc {
            pid: 502,
            comm: "opencode".to_string(),
            cwd: Some("/home/gabe/.local/share/farmerbob/worktrees/wt-shared".to_string()),
        };
        assert_eq!(count_live(&[p1, p2], root), 2);
    }

    #[test]
    fn clause_6_unreadable_order_and_no_dedup() {
        let p1 = Proc {
            pid: 603,
            comm: "opencode".to_string(),
            cwd: None,
        };
        let p2 = Proc {
            pid: 601,
            comm: "zcode".to_string(),
            cwd: Some("/some/path".to_string()),
        };
        let p3 = Proc {
            pid: 603,
            comm: "agy".to_string(),
            cwd: None,
        };
        let p4 = Proc {
            pid: 602,
            comm: "codex".to_string(),
            cwd: None,
        };
        assert_eq!(unreadable(&[p1, p2, p3, p4]), vec![603, 603, 602]);
    }

    #[test]
    fn clause_7_run_exits_zero() {
        assert_eq!(run(&[]), 0);
        assert_eq!(
            run(&[
                String::from("--root"),
                String::from("/path/that/does/not/exist")
            ]),
            0
        );
    }

    #[test]
    fn boundary_procs_empty() {
        let root = "/home/gabe/.local/share/farmerbob/worktrees";
        assert_eq!(count_live(&[], root), 0);
        assert!(unreadable(&[]).is_empty());
    }

    #[test]
    fn boundary_all_unreadable() {
        let root = "/home/gabe/.local/share/farmerbob/worktrees";
        let p1 = Proc {
            pid: 801,
            comm: "opencode".to_string(),
            cwd: None,
        };
        let p2 = Proc {
            pid: 802,
            comm: "agy".to_string(),
            cwd: None,
        };
        assert_eq!(count_live(&[p1.clone(), p2.clone()], root), 0);
        assert_eq!(unreadable(&[p1, p2]), vec![801, 802]);
    }

    #[test]
    fn boundary_root_empty_string() {
        let p1 = Proc {
            pid: 901,
            comm: "agy".to_string(),
            cwd: Some("/home/gabe/.local/share/farmerbob/worktrees/wt-1".to_string()),
        };
        let p2 = Proc {
            pid: 902,
            comm: "codex".to_string(),
            cwd: Some("relative/worktree".to_string()),
        };
        assert_eq!(count_live(&[p1, p2], ""), 0);
    }

    #[test]
    fn boundary_cwd_trailing_slash() {
        let root = "/home/gabe/.local/share/farmerbob/worktrees";
        let p1 = Proc {
            pid: 1001,
            comm: "agy".to_string(),
            cwd: Some("/home/gabe/.local/share/farmerbob/worktrees/wt-1".to_string()),
        };
        let p2 = Proc {
            pid: 1002,
            comm: "agy".to_string(),
            cwd: Some("/home/gabe/.local/share/farmerbob/worktrees/wt-1/".to_string()),
        };
        assert_eq!(count_live(&[p1], root), 1);
        assert_eq!(count_live(&[p2], root), 1);
    }

    #[test]
    fn boundary_comm_not_consulted_by_count_live() {
        let root = "/home/gabe/.local/share/farmerbob/worktrees";
        let p = Proc {
            pid: 1101,
            comm: "unrelated_custom_binary".to_string(),
            cwd: Some("/home/gabe/.local/share/farmerbob/worktrees/wt-1".to_string()),
        };
        assert_eq!(count_live(&[p], root), 1);
    }

    #[test]
    fn composition_exhaustive_and_disjoint() {
        let root = "/home/gabe/.local/share/farmerbob/worktrees";
        let p1 = Proc {
            pid: 1201,
            comm: "agy".to_string(),
            cwd: Some("/home/gabe/.local/share/farmerbob/worktrees/wt-1".to_string()),
        };
        let p2 = Proc {
            pid: 1202,
            comm: "opencode".to_string(),
            cwd: None,
        };
        let p3 = Proc {
            pid: 1203,
            comm: "codex".to_string(),
            cwd: Some("/tmp/outside".to_string()),
        };
        let p4 = Proc {
            pid: 1204,
            comm: "zcode".to_string(),
            cwd: Some("/home/gabe/.local/share/farmerbob/worktrees".to_string()),
        };

        let procs = vec![p1, p2, p3, p4];
        let live_count = count_live(&procs, root);
        let unreadable_pids = unreadable(&procs);
        let outside_count = procs
            .iter()
            .filter(|p| {
                let Some(cwd) = &p.cwd else {
                    return false;
                };
                let cwd_path = Path::new(cwd);
                match cwd_path.strip_prefix(Path::new(root)) {
                    Ok(rel) => !rel.components().any(|c| matches!(c, Component::Normal(_))),
                    Err(_) => true,
                }
            })
            .count();

        assert_eq!(live_count, 1);
        assert_eq!(unreadable_pids, vec![1202]);
        assert_eq!(outside_count, 2);
        assert_eq!(
            live_count + unreadable_pids.len() + outside_count,
            procs.len()
        );
    }
}
