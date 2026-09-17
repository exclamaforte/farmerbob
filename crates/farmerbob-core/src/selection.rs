//! Winner selection and loser sweeping for parallel agent runs.
//!
//! When N agents attack one task, farmerbob promotes one winner, merges it, and
//! sweeps the rest. The rejected candidates are not garbage: each is a labelled
//! negative preserved in an archive, the corpus used to measure whether a reviewer
//! can detect a defect. This module decides, purely from supplied data, how a
//! conflict classifies, where a swept loser is preserved, which losers to sweep,
//! and whether a candidate may be promoted.
//!
//! All functions are pure: they perform no I/O and invoke no git. Diffs and
//! conflicts are passed in as data.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// A reference to a candidate (one agent's result for a task).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateRef(pub String);

/// A git ref under which a swept loser is preserved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveRef(pub String);

/// One conflicting region: the lines each side placed there.
///
/// A conflict region carries only what each side contributed; it does not carry
/// the base. Classification therefore reasons about the two line sets directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    /// Path of the file containing the conflict.
    pub path: String,
    /// Lines our side contributed to the region.
    pub ours: Vec<String>,
    /// Lines their side contributed to the region.
    pub theirs: Vec<String>,
}

/// The resolution farmerbob should apply to a [`Conflict`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resolution {
    /// Both sides only inserted distinct lines; the union is correct. `lines` is
    /// the merged set, deduplicated and in a deterministic (sorted) order.
    Union { lines: Vec<String> },
    /// Anything else. A human or the planning agent must decide.
    Escalate { reason: String },
}

/// Why a selection operation failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectionError {
    /// The candidate's result was quarantined; promoting it needs an explicit override.
    Quarantined { candidate: CandidateRef },
    /// The named candidate is not among the candidates under consideration.
    UnknownCandidate { candidate: CandidateRef },
}

/// Classify a conflict.
///
/// Returns [`Resolution::Union`] only when both sides are pure insertions of
/// distinct lines: neither side removed a line the other kept (so neither region
/// is empty while the other is not) and the two line sets are disjoint. Any
/// shared line, any removal, or any other shape is a real conflict and returns
/// [`Resolution::Escalate`].
///
/// The union output is deterministic: identical input always yields identical
/// output, because the merged lines are emitted in sorted order.
pub fn resolve(conflict: &Conflict) -> Resolution {
    let ours: BTreeSet<&str> = conflict.ours.iter().map(String::as_str).collect();
    let theirs: BTreeSet<&str> = conflict.theirs.iter().map(String::as_str).collect();

    // A removal by one side: the other side kept lines we dropped. Not additive.
    if ours.is_empty() || theirs.is_empty() {
        return Resolution::Escalate {
            reason: "one side removed lines the other kept".to_string(),
        };
    }

    // Any shared line means both sides touched the same content: a real edit
    // conflict, not two independent insertions. Escalate even when the shared
    // line is identical on both sides.
    if ours.intersection(&theirs).next().is_some() {
        return Resolution::Escalate {
            reason: "both sides touched a shared line".to_string(),
        };
    }

    // Pure disjoint insertions. BTreeSet iteration is already sorted, giving a
    // deterministic, deduplicated merge.
    let mut lines: Vec<String> = ours.union(&theirs).map(|s| s.to_string()).collect();
    lines.sort();
    Resolution::Union { lines }
}

/// Sanitise a single component the way a git ref / branch name would be, so it
/// is safe to embed in `refs/fb/archive/<task>/<candidate>`.
///
/// Forbidden: spaces, `~`, `^`, `:`, `?`, `*`, `\`, `[`, `]`, control
/// characters, the substring `..`, a leading or trailing `/` or `.`, and a
/// trailing `.lock`. Illegal runs are collapsed to a single `-`.
fn sanitize_ref_segment(raw: &str) -> String {
    let mut out = String::new();
    let mut pending_sep = false;
    for ch in raw.chars() {
        let bad =
            ch.is_control() || matches!(ch, ' ' | '~' | '^' | ':' | '?' | '*' | '\\' | '[' | ']');
        if bad {
            if !pending_sep && !out.is_empty() {
                out.push('-');
                pending_sep = true;
            }
        } else {
            out.push(ch);
            pending_sep = false;
        }
    }

    // Strip leading/trailing '/' and '.'; collapse any resulting "..".
    let mut res = out;
    loop {
        let trimmed: String = res.trim_matches(|c| c == '/' || c == '.').to_string();
        let collapsed = trimmed.replace("..", ".");
        if collapsed == res {
            res = collapsed;
            break;
        }
        res = collapsed;
    }

    // A ref must not end in ".lock".
    if res.ends_with(".lock") {
        res.truncate(res.len() - ".lock".len());
        if res.is_empty() || res.ends_with('.') {
            res.push('-');
        }
    }

    if res.is_empty() {
        res.push('-');
    }
    res
}

/// The archive ref a swept loser is preserved under: `refs/fb/archive/<task>/<candidate>`.
///
/// The candidate name is sanitised the same way a branch name would be. `task` is
/// taken verbatim; callers are expected to pass an already-valid task identifier.
pub fn archive_ref(task: &str, candidate: &CandidateRef) -> ArchiveRef {
    let safe = sanitize_ref_segment(&candidate.0);
    ArchiveRef(format!("refs/fb/archive/{task}/{safe}"))
}

/// Which candidates to sweep: everything except the winner, in a stable (input)
/// order. Errors if the winner is not among the candidates.
pub fn sweep_list(
    candidates: &[CandidateRef],
    winner: &CandidateRef,
) -> Result<Vec<CandidateRef>, SelectionError> {
    if !candidates.iter().any(|c| c == winner) {
        return Err(SelectionError::UnknownCandidate {
            candidate: winner.clone(),
        });
    }
    let swept: Vec<CandidateRef> = candidates
        .iter()
        .filter(|c| c != &winner)
        .cloned()
        .collect();
    Ok(swept)
}

/// Whether a candidate may be promoted. `quarantined` marks results whose
/// measurement was contaminated; those require `force` to be promoted.
pub fn may_promote(
    candidate: &CandidateRef,
    quarantined: &[CandidateRef],
    force: bool,
) -> Result<(), SelectionError> {
    if !force && quarantined.iter().any(|c| c == candidate) {
        return Err(SelectionError::Quarantined {
            candidate: candidate.clone(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_mod_lines_union() {
        let conflict = Conflict {
            path: "src/main.rs".to_string(),
            ours: vec!["mod a;".to_string()],
            theirs: vec!["mod b;".to_string()],
        };
        match resolve(&conflict) {
            Resolution::Union { lines } => {
                assert_eq!(lines, vec!["mod a;".to_string(), "mod b;".to_string()]);
            }
            other => panic!("expected union, got {other:?}"),
        }
    }

    #[test]
    fn modification_on_one_side_escalates() {
        // One side rewrote a function body; the other kept the same line as
        // context. The shared line makes this a real edit conflict.
        let conflict = Conflict {
            path: "src/lib.rs".to_string(),
            ours: vec!["fn f() { 1 }".to_string(), "fn g() {}".to_string()],
            theirs: vec!["fn g() {}".to_string(), "fn f() { 2 }".to_string()],
        };
        assert!(matches!(resolve(&conflict), Resolution::Escalate { .. }));
    }

    #[test]
    fn identical_lines_escalate_rather_than_union() {
        let conflict = Conflict {
            path: "src/lib.rs".to_string(),
            ours: vec!["mod shared;".to_string()],
            theirs: vec!["mod shared;".to_string()],
        };
        assert!(matches!(resolve(&conflict), Resolution::Escalate { .. }));
    }

    #[test]
    fn union_ordering_is_stable_across_runs() {
        let conflict = Conflict {
            path: "f.rs".to_string(),
            ours: vec!["zeta;".to_string(), "alpha;".to_string()],
            theirs: vec!["mu;".to_string(), "charlie;".to_string()],
        };
        let first = resolve(&conflict);
        for _ in 0..50 {
            assert_eq!(resolve(&conflict), first);
        }
        match first {
            Resolution::Union { lines } => {
                assert_eq!(
                    lines,
                    vec![
                        "alpha;".to_string(),
                        "charlie;".to_string(),
                        "mu;".to_string(),
                        "zeta;".to_string()
                    ]
                );
            }
            other => panic!("expected union, got {other:?}"),
        }
    }

    #[test]
    fn removal_on_one_side_escalates() {
        let conflict = Conflict {
            path: "f.rs".to_string(),
            ours: vec![],
            theirs: vec!["kept;".to_string()],
        };
        assert!(matches!(resolve(&conflict), Resolution::Escalate { .. }));
    }

    #[test]
    fn archive_ref_five_nasty_inputs() {
        let task = "T1";

        let a = archive_ref(task, &CandidateRef("feature/foo bar".to_string()));
        assert_eq!(a.0, "refs/fb/archive/T1/feature/foo-bar");

        let b = archive_ref(task, &CandidateRef("a..b".to_string()));
        assert_eq!(b.0, "refs/fb/archive/T1/a.b");

        let c = archive_ref(task, &CandidateRef("/leading".to_string()));
        assert_eq!(c.0, "refs/fb/archive/T1/leading");

        let d = archive_ref(task, &CandidateRef("trailing/".to_string()));
        assert_eq!(d.0, "refs/fb/archive/T1/trailing");

        let e = archive_ref(task, &CandidateRef("bad~^:?*\\[x].lock".to_string()));
        assert_eq!(e.0, "refs/fb/archive/T1/bad-x-");
    }

    #[test]
    fn archive_ref_never_ends_in_lock() {
        let r = archive_ref("T", &CandidateRef("ends.lock".to_string()));
        assert!(!r.0.ends_with(".lock"));
        assert_eq!(r.0, "refs/fb/archive/T/ends");
    }

    #[test]
    fn sweep_list_winner_absent_errors() {
        let candidates = vec![CandidateRef("a".to_string()), CandidateRef("b".to_string())];
        let winner = CandidateRef("missing".to_string());
        match sweep_list(&candidates, &winner) {
            Err(SelectionError::UnknownCandidate { candidate }) => {
                assert_eq!(candidate, winner);
            }
            other => panic!("expected UnknownCandidate, got {other:?}"),
        }
    }

    #[test]
    fn sweep_list_omits_winner_and_keeps_order() {
        let candidates = vec![
            CandidateRef("a".to_string()),
            CandidateRef("win".to_string()),
            CandidateRef("c".to_string()),
            CandidateRef("d".to_string()),
        ];
        let winner = CandidateRef("win".to_string());
        let swept = sweep_list(&candidates, &winner).expect("winner present");
        assert_eq!(
            swept,
            vec![
                CandidateRef("a".to_string()),
                CandidateRef("c".to_string()),
                CandidateRef("d".to_string())
            ]
        );
        assert!(!swept.contains(&winner));
    }

    #[test]
    fn quarantined_candidate_needs_force() {
        let candidate = CandidateRef("q".to_string());
        let quarantined = vec![candidate.clone()];

        assert!(matches!(
            may_promote(&candidate, &quarantined, false),
            Err(SelectionError::Quarantined { .. })
        ));
        assert!(may_promote(&candidate, &quarantined, true).is_ok());

        let clean = CandidateRef("clean".to_string());
        assert!(may_promote(&clean, &quarantined, false).is_ok());
    }
}
