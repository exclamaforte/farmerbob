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

/// How many agents are working in a worktree.
///
/// COUNT WORKTREES, NOT PROCESSES. One run occupies one worktree and one admission slot,
/// and it is a whole process tree: the wrapper, bwrap, the launcher CLI and however many
/// node workers it spawns -- seven of them for a single codex run. Counting processes
/// answers a question no caller asked and inflates by the arity of whatever launcher is in
/// use.
///
/// `root` is the worktree root, e.g. `/home/gabe/.local/share/farmerbob/worktrees`.
pub fn count_live(procs: &[Proc], root: &str) -> usize {
    if root.is_empty() {
        return 0;
    }
    worktrees_live(procs, root).len()
}

/// The worktrees with at least one live process, by name.
pub fn worktrees_live(procs: &[Proc], root: &str) -> std::collections::BTreeSet<String> {
    let mut seen = std::collections::BTreeSet::new();
    if root.is_empty() {
        return seen;
    }
    let root_path = Path::new(root);
    for p in procs {
        if is_harness_tool(&p.comm) {
            continue;
        }
        let Some(cwd) = &p.cwd else { continue };
        let Ok(rel) = Path::new(cwd).strip_prefix(root_path) else {
            continue;
        };
        if let Some(Component::Normal(first)) = rel.components().next() {
            seen.insert(first.to_string_lossy().into_owned());
        }
    }
    seen
}

/// The processes whose working directory could not be read, so a caller can
/// say how many it could not classify rather than silently undercounting.
pub fn unreadable(procs: &[Proc]) -> Vec<u32> {
    procs
        .iter()
        .filter(|p| p.cwd.is_none() && looks_like_a_launcher(&p.comm))
        .map(|p| p.pid)
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

    let procs = gather_procs(&root);
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
pub fn gather_procs(root: &str) -> Vec<Proc> {
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
        // NO NAME FILTER. This matched a fixed list -- opencode, zcode, agy, codex -- which
        // was a second copy of the launcher table, kept by hand and already wrong: the
        // zcode launcher runs as `zcode-cli`, so every glm arm was invisible and `fb live`
        // reported 1 against two running scopes. A count used for ADMISSION that silently
        // undercounts lets the harness dispatch past its own memory cap.
        //
        // The worktree is the ground truth, as this module's own heading says. A process is
        // an agent's if its working directory is inside one, whatever it calls itself.
        let cwd = std::fs::read_link(format!("/proc/{pid}/cwd"))
            .ok()
            .map(|p| p.to_string_lossy().into_owned());
        let inside = cwd.as_deref().is_some_and(|c| c.starts_with(root));
        if inside || cwd.is_none() {
            procs.push(Proc { pid, comm, cwd });
        }
    }
    procs.sort_by_key(|p| p.pid);
    procs
}

/// Returns true if the executable name matches candidate agent binaries.
/// Whether a process is the HARNESS's own tooling rather than an agent.
///
/// EXCLUSION, NOT INCLUSION, AND THAT ASYMMETRY IS THE POINT. This module used to hold a
/// list of launcher names and count only those; the list was a hand-kept second copy of the
/// launcher table and was already wrong -- the zcode launcher runs as `zcode-cli`, so every
/// glm arm was invisible and the count used for ADMISSION under-reported, which lets the
/// harness dispatch past its own memory cap.
///
/// An inclusion list fails UNSAFE: a launcher nobody added is an agent nobody counts.
/// An exclusion list fails SAFE: a tool nobody added is counted as an agent, which
/// over-reports and holds a slot back. Over-reporting costs throughput; under-reporting
/// costs the machine.
///
/// What it excludes is the scorer, which runs `cargo test` INSIDE the candidate's worktree.
/// `fb score` on blast-width made that finished worktree read as a live agent for the whole
/// of a four-minute compile, eight minutes after its arm had exited.
///
/// A cargo test binary is named `<crate>-<16 hex digits>`, which is why the hash shape is
/// matched rather than any particular crate name.
fn is_harness_tool(comm: &str) -> bool {
    const TOOLS: [&str; 7] = [
        "cargo",
        "rustc",
        "rustfmt",
        "clippy-driver",
        "git",
        "bwrap",
        "fb",
    ];
    if TOOLS.contains(&comm) {
        return true;
    }
    // A cargo test binary is `<crate>-<16 hex digits>`. `comm` is TRUNCATED TO 15 BYTES by
    // the kernel, so what actually appears for `farmerbob_core-1b360807fb9539bc` is
    // `farmerbob_core-` -- the hash cut off entirely, leaving a trailing dash. Requiring a
    // hash matched nothing, and the scorer went on counting as an agent.
    //
    // A trailing dash is therefore the truncated case, and no launcher is named that way.
    if comm.ends_with('-') {
        return true;
    }
    match comm.rsplit_once('-') {
        Some((stem, hash)) if !stem.is_empty() && !hash.is_empty() => {
            // A full hash, or a partial one on a name the kernel truncated at 15 bytes.
            hash.chars().all(|c| c.is_ascii_hexdigit()) && (hash.len() >= 8 || comm.len() == 15)
        }
        _ => false,
    }
}

/// Launcher names seen in this project, used ONLY to decide whether an unreadable process
/// is worth reporting as unclassified. It never decides what counts as live -- that is the
/// worktree.
fn looks_like_a_launcher(comm: &str) -> bool {
    ["opencode", "zcode", "agy", "codex", "claude", "ori", "node"]
        .iter()
        .any(|n| comm.starts_with(n))
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

    /// Clause 5, REVERSED on 2026-09-19. It read "counts processes, not distinct worktrees
    /// ... because the caller uses this against a slot budget and slots are per process".
    /// That was false about the budget -- `admit_cmd::usable_slots` divides available memory
    /// by a PER-RUN figure -- and it only survived because a hardcoded comm filter admitted
    /// one process per run. With the filter gone one codex run is seven processes, which
    /// would report seven agents against a budget that is full at one.
    #[test]
    fn clause_5_counts_worktrees_not_processes() {
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
        assert_eq!(count_live(&[p1, p2], root), 1);
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

    /// ONE RUN IS ONE AGENT, however many processes it is. A single codex run is seven --
    /// the wrapper, bwrap, the CLI and four node workers -- and counting processes answers
    /// a question no caller asked while inflating by the arity of the launcher in use.
    #[test]
    fn a_whole_process_tree_in_one_worktree_counts_once() {
        let root = "/wt";
        let tree: Vec<Proc> = ["fb", "bwrap", "codex", "node_repl", "node_repl"]
            .iter()
            .enumerate()
            .map(|(i, c)| Proc {
                pid: 100 + i as u32,
                comm: c.to_string(),
                cwd: Some("/wt/task--arm".to_string()),
            })
            .collect();
        assert_eq!(count_live(&tree, root), 1);
    }

    /// The name filter was a SECOND COPY of the launcher table, kept by hand, and it was
    /// already wrong: the zcode launcher runs as `zcode-cli`, not `zcode`, so every glm arm
    /// was invisible and `fb live` reported 1 against two running scopes. A count used for
    /// ADMISSION that silently undercounts lets the harness dispatch past its memory cap.
    #[test]
    fn an_agent_is_counted_whatever_its_process_calls_itself() {
        let root = "/wt";
        let procs = vec![
            Proc {
                pid: 1,
                comm: "zcode-cli".into(),
                cwd: Some("/wt/a--glm".into()),
            },
            Proc {
                pid: 2,
                comm: "codex-code-mode".into(),
                cwd: Some("/wt/b--codex".into()),
            },
            Proc {
                pid: 3,
                comm: "something-nobody-registered".into(),
                cwd: Some("/wt/c--new".into()),
            },
        ];
        assert_eq!(count_live(&procs, root), 3);
        assert_eq!(
            worktrees_live(&procs, root).into_iter().collect::<Vec<_>>(),
            vec!["a--glm", "b--codex", "c--new"]
        );
    }

    /// A process sitting at the root itself is in no worktree, so it is no agent.
    #[test]
    fn a_process_at_the_root_itself_is_not_an_agent() {
        let procs = vec![Proc {
            pid: 1,
            comm: "fb".into(),
            cwd: Some("/wt".into()),
        }];
        assert_eq!(count_live(&procs, "/wt"), 0);
    }

    /// An unreadable cwd is NOT "not an agent" -- it is a process that could not be
    /// classified, and the caller is told rather than quietly undercounted.
    #[test]
    fn an_unclassifiable_launcher_is_reported_not_swallowed() {
        let procs = vec![
            Proc {
                pid: 7,
                comm: "zcode-cli".into(),
                cwd: None,
            },
            Proc {
                pid: 8,
                comm: "sshd".into(),
                cwd: None,
            },
        ];
        assert_eq!(
            unreadable(&procs),
            vec![7],
            "only a plausible launcher is worth reporting"
        );
    }

    /// An empty root cannot be matched against, and answering 0 for it would read as "no
    /// agents" rather than "nothing was asked".
    #[test]
    fn an_empty_root_matches_nothing() {
        let procs = vec![Proc {
            pid: 1,
            comm: "codex".into(),
            cwd: Some("/wt/a".into()),
        }];
        assert_eq!(count_live(&procs, ""), 0);
        assert!(worktrees_live(&procs, "").is_empty());
    }

    /// THE SCORER RUNS INSIDE THE CANDIDATE'S WORKTREE. `fb score` runs `cargo test` there,
    /// so a finished worktree read as a live agent for the whole of a four-minute compile --
    /// eight minutes after its arm had exited. That over-counts the field, and the autopilot
    /// holds slots back for agents that are not there.
    #[test]
    fn the_scorers_own_cargo_is_not_an_agent() {
        let root = "/wt";
        let procs = vec![
            Proc {
                pid: 1,
                comm: "cargo".into(),
                cwd: Some("/wt/task--arm".into()),
            },
            Proc {
                pid: 2,
                comm: "farmerbob_core-1b360807fb95".into(),
                cwd: Some("/wt/task--arm".into()),
            },
        ];
        assert_eq!(count_live(&procs, root), 0, "only the scorer is in there");
    }

    /// The exclusion must not swallow a real agent that happens to sit beside the scorer.
    #[test]
    fn an_agent_beside_the_scorer_is_still_counted() {
        let root = "/wt";
        let procs = vec![
            Proc {
                pid: 1,
                comm: "cargo".into(),
                cwd: Some("/wt/a--x".into()),
            },
            Proc {
                pid: 2,
                comm: "zcode-cli".into(),
                cwd: Some("/wt/b--y".into()),
            },
        ];
        assert_eq!(count_live(&procs, root), 1);
        assert_eq!(
            worktrees_live(&procs, root).into_iter().collect::<Vec<_>>(),
            vec!["b--y"]
        );
    }

    /// EXCLUSION FAILS SAFE. A tool nobody added is counted as an agent, which holds a slot
    /// back; an INCLUSION list would miss it and let the harness dispatch past its cap. The
    /// previous design was an inclusion list and it was already wrong about `zcode-cli`.
    #[test]
    fn an_unknown_process_counts_as_an_agent() {
        assert!(!is_harness_tool("some-new-launcher"));
        assert!(!is_harness_tool("zcode-cli"));
        assert!(!is_harness_tool("codex-code-mode"));
    }

    /// A cargo test binary is `<crate>-<hex>`; an arm's CLI is not.
    #[test]
    fn a_test_binary_is_told_apart_from_a_launcher() {
        assert!(is_harness_tool("farmerbob_core-1b360807fb95"));
        assert!(is_harness_tool("fb-1b360807fb9539bc"));
        // THE TRUNCATED CASE, which is the one that actually appears. `comm` is capped at
        // 15 bytes, so `farmerbob_core-1b360807fb9539bc` shows up as `farmerbob_core-`
        // with the hash gone. Requiring a hash matched nothing and the scorer kept counting.
        assert!(is_harness_tool("farmerbob_core-"));
        assert!(
            !is_harness_tool("zcode-node-repl"),
            "not hex, not a test binary"
        );
        assert!(!is_harness_tool("agy-opus"), "too short to be a hash");
    }
}
