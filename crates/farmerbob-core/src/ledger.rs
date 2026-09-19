//! Credit for finding the flaw in the question, and for naming work nobody asked about.
//!
//! Two stages feed this module. Before any implementation runs, arms critique the SPEC and
//! return `FINDINGS`; during cross-critique they may additionally return `FOLLOWUPS`, work
//! worth doing in the codebase that is out of scope for the patch under review. The
//! adjudicator rules on each. An ACCEPTED proposal is a positive signal about the arm that
//! made it, distinct from implementing well and not measured by anything else here.
//!
//! The motivating evidence is in `docs/PROPOSALS.md`: five consecutive merged tasks each
//! produced a finding about the specification rather than about any arm's code, every one of
//! them after three arms had already implemented against it. The information existed before
//! the work did and nothing asked for it.
//!
//! This module is pure. It parses the two output contracts and reduces rulings to counts; it
//! reads no files and makes no judgements.

use crate::measurement::Measurement;

/// Which stage a proposal came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A defect in the specification, raised before implementation.
    SpecDefect,
    /// Work worth doing in the codebase, out of scope for the patch reviewed.
    FollowUp,
}

/// What the adjudicator decided about one proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ruling {
    /// Acted on: the spec was revised, or a bead was filed.
    Accepted,
    /// Not acted on.
    Rejected,
    /// Correct, but already known -- an existing bead, or another arm got
    /// there first in the same round.
    ///
    /// Deliberately NOT [`Ruling::Rejected`]. An arm that independently finds
    /// something already recorded was right; it simply was not first, and
    /// counting that as a rejection would punish being correct.
    Duplicate,
}

/// One proposal, as ruled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The arm that made it.
    pub arm: String,
    /// The task it was made during.
    pub task: String,
    /// Which stage.
    pub kind: Kind,
    /// The adjudicator's ruling.
    pub ruling: Ruling,
    /// One line, as the arm wrote it.
    pub title: String,
}

/// Per-arm totals over some set of entries.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Credit {
    /// The arm.
    pub arm: String,
    /// Spec defects accepted.
    pub spec_accepted: u32,
    /// Spec defects ruled duplicate -- correct but not first.
    pub spec_duplicate: u32,
    /// Spec defects proposed in total, including rejected.
    pub spec_proposed: u32,
    /// Follow-ups accepted.
    pub followups_accepted: u32,
    /// Follow-ups ruled duplicate.
    pub followups_duplicate: u32,
    /// Follow-ups proposed in total.
    pub followups_proposed: u32,
}

/// Reduce ruled entries to per-arm credits, sorted by arm name.
///
/// Every entry is counted. An arm with no entries does not appear: this
/// returns what the record contains, not a row per known arm, because
/// "proposed nothing" and "is not in this record" are different facts and
/// only the caller knows which arms it asked.
pub fn tally(entries: &[Entry]) -> Vec<Credit> {
    let mut out: Vec<Credit> = Vec::new();
    for e in entries {
        let idx = match out.iter().position(|c| c.arm == e.arm) {
            Some(i) => i,
            None => {
                out.push(Credit {
                    arm: e.arm.clone(),
                    ..Credit::default()
                });
                out.len() - 1
            }
        };
        let c = &mut out[idx];
        match e.kind {
            Kind::SpecDefect => {
                c.spec_proposed = c.spec_proposed.saturating_add(1);
                match e.ruling {
                    Ruling::Accepted => c.spec_accepted = c.spec_accepted.saturating_add(1),
                    Ruling::Duplicate => c.spec_duplicate = c.spec_duplicate.saturating_add(1),
                    Ruling::Rejected => {}
                }
            }
            Kind::FollowUp => {
                c.followups_proposed = c.followups_proposed.saturating_add(1);
                match e.ruling {
                    Ruling::Accepted => {
                        c.followups_accepted = c.followups_accepted.saturating_add(1)
                    }
                    Ruling::Duplicate => {
                        c.followups_duplicate = c.followups_duplicate.saturating_add(1)
                    }
                    Ruling::Rejected => {}
                }
            }
        }
    }
    out.sort_by(|a, b| a.arm.cmp(&b.arm));
    out
}

/// The share of an arm's proposals of one kind that were acted on.
///
/// `Duplicate` counts toward the numerator: it means correct-but-not-first,
/// not wrong.
///
/// Returns [`Measurement::Missing`] when the arm proposed nothing of that
/// kind. A zero here would be indistinguishable from an arm that proposed ten
/// and had all ten rejected, which is the failure state this crate refuses to
/// make look like a success state.
pub fn acceptance_rate(c: &Credit, kind: Kind) -> Measurement<f64> {
    let (good, total) = match kind {
        Kind::SpecDefect => (c.spec_accepted + c.spec_duplicate, c.spec_proposed),
        Kind::FollowUp => (
            c.followups_accepted + c.followups_duplicate,
            c.followups_proposed,
        ),
    };
    if total == 0 {
        return Measurement::nothing_to_measure("the arm proposed nothing of this kind");
    }
    Measurement::observed(f64::from(good) / f64::from(total))
}

/// A proposal as an arm wrote it, before any ruling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    /// Which stage's contract it was parsed from.
    pub kind: Kind,
    /// The one-line statement following the marker.
    pub title: String,
    /// The lines beneath it, up to the next marker, trimmed of surrounding
    /// blank lines. May be empty.
    pub body: String,
}

/// Where an accepted proposal's work goes.
///
/// This is a RULING, not a computation. Nothing in this module derives it, and that is
/// deliberate: the question is how much work the follow-up is, and no function here can
/// answer that. `scope_of` and [`evidence`] gather what a judge needs; a judge decides.
///
/// It was derived once, from whether the named paths fell inside the reviewed task's
/// declared set. That rule is confidently wrong in both directions. Three follow-ups from
/// one afternoon:
///
/// - `labelled_count` -- same file, one turn. Membership said in-turn. Right.
/// - the `pareto.rs` migration -- different file, twenty call sites. Membership said task.
///   Right, and it was a duplicate of a bead filed weeks earlier.
/// - `Tokens` needs a fourth field -- SAME file, and an API redesign contradicting a pinned
///   clause of the spec. Membership said in-turn. Wrong, and it is the expensive one: it
///   needed a spec revision and a run, and was instead done inline in a merge commit.
///
/// Files do not predict effort. A one-line change in a foreign file is trivial; a redesign
/// in the file under review is a week.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Small enough to hand back to the arm that wrote the code, in one turn, in the
    /// worktree it still owns. The cheap path: the author has the file, the task and its
    /// own reasoning already loaded, and nothing becomes a bead.
    InTurn,
    /// Needs its own spec and its own run -- a refactor, a new type, a decision the author
    /// cannot make alone, or anything that changes what the spec pinned.
    Task,
    /// Nothing can be decided about it as written.
    Unroutable,
}

/// What an adjudicator needs in order to rule on a follow-up.
///
/// Every field here is GATHERED. None of it is a verdict, and the struct deliberately
/// carries no method that returns a [`Route`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    /// The paths the follow-up names, in the order written. Empty when it named none,
    /// which is the one case a judge can dispose of without reading anything else.
    pub scope: Vec<String>,
    /// Those the reviewed task declared. A judge reads this as "the author is already
    /// here", not as "therefore in-turn".
    pub inside: Vec<String>,
    /// Those it did not. A judge reads this as "somebody else owns this file", not as
    /// "therefore a task".
    pub outside: Vec<String>,
    /// Whether the author's worktree still exists. When it does not, an in-turn ruling is
    /// not available whatever the work costs -- there is nothing to resume into.
    pub author_worktree: bool,
}

/// Gather what a judge needs. Decides nothing.
///
/// `declared` is what the reviewed task declared; `author_worktree` is whether the arm's
/// worktree is still on disk. Paths are compared as exact strings, the same way
/// `crate::scope` compares them.
pub fn evidence(proposal: &Proposal, declared: &[String], author_worktree: bool) -> Evidence {
    let scope = scope_of(proposal);
    let (inside, outside): (Vec<String>, Vec<String>) = scope
        .iter()
        .cloned()
        .partition(|path| declared.iter().any(|d| d == path));
    Evidence {
        scope,
        inside,
        outside,
        author_worktree,
    }
}

/// The paths a proposal's `SCOPE:` line names, in the order written.
///
/// A scope line may name several paths separated by commas or whitespace. Returns empty
/// when the proposal has no `SCOPE:` line.
pub fn scope_of(proposal: &Proposal) -> Vec<String> {
    for line in proposal.body.lines() {
        let Some(rest) = line.trim_start().strip_prefix("SCOPE:") else {
            continue;
        };
        return rest
            .split([',', ' ', '\t'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
    }
    Vec::new()
}

/// Parse the `FINDING:` / `FOLLOWUP:` blocks out of an arm's report.
///
/// A marker is recognised only at the START of a line, after optional leading
/// whitespace -- the same rule `precondition` pins, and for the same reason:
/// this project has four separate bugs caused by a scanner matching its own
/// syntax quoted inside prose, and these prompts describe both markers by
/// name.
///
/// A marker with an empty title is not a proposal and is skipped, not an
/// error. Returns proposals in document order.
pub fn parse(report: &str) -> Vec<Proposal> {
    let mut out: Vec<Proposal> = Vec::new();
    let mut current: Option<Proposal> = None;
    let mut body: Vec<&str> = Vec::new();
    for line in report.lines() {
        let t = line.trim_start();
        let found = if let Some(rest) = t.strip_prefix("FINDING:") {
            Some((Kind::SpecDefect, rest))
        } else {
            t.strip_prefix("FOLLOWUP:")
                .map(|rest| (Kind::FollowUp, rest))
        };
        if let Some((kind, rest)) = found {
            if let Some(mut p) = current.take() {
                p.body = body.join("\n").trim().to_string();
                out.push(p);
            }
            body.clear();
            let title = rest.trim();
            current = if title.is_empty() {
                None
            } else {
                Some(Proposal {
                    kind,
                    title: title.to_string(),
                    body: String::new(),
                })
            };
        } else if current.is_some() {
            body.push(line);
        }
    }
    if let Some(mut p) = current.take() {
        p.body = body.join("\n").trim().to_string();
        out.push(p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(arm: &str, kind: Kind, ruling: Ruling) -> Entry {
        Entry {
            arm: arm.into(),
            task: "t".into(),
            kind,
            ruling,
            title: "x".into(),
        }
    }

    #[test]
    fn tally_is_empty_for_no_entries() {
        assert_eq!(tally(&[]), vec![]);
    }

    #[test]
    fn tally_sorts_by_arm_and_counts_each_kind_separately() {
        let t = tally(&[
            e("zeta", Kind::SpecDefect, Ruling::Accepted),
            e("alpha", Kind::FollowUp, Ruling::Accepted),
            e("alpha", Kind::SpecDefect, Ruling::Rejected),
        ]);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].arm, "alpha");
        assert_eq!(t[1].arm, "zeta");
        assert_eq!(t[0].followups_accepted, 1);
        assert_eq!(t[0].spec_proposed, 1);
        assert_eq!(t[0].spec_accepted, 0);
    }

    /// Correct but not first is not the same as wrong.
    #[test]
    fn duplicate_counts_toward_the_rate_and_rejected_does_not() {
        let t = tally(&[
            e("a", Kind::SpecDefect, Ruling::Accepted),
            e("a", Kind::SpecDefect, Ruling::Duplicate),
            e("a", Kind::SpecDefect, Ruling::Rejected),
        ]);
        let c = &t[0];
        assert_eq!(c.spec_proposed, 3);
        assert_eq!(c.spec_accepted, 1);
        assert_eq!(c.spec_duplicate, 1);
        match acceptance_rate(c, Kind::SpecDefect) {
            Measurement::Observed(r) => assert!((r - 2.0 / 3.0).abs() < 1e-9, "{r}"),
            other => panic!("expected a rate, got {other:?}"),
        }
    }

    /// The whole point of returning a Measurement: proposing nothing is not
    /// the same as proposing badly.
    #[test]
    fn an_arm_that_proposed_nothing_has_no_rate_rather_than_zero() {
        let c = Credit {
            arm: "a".into(),
            ..Credit::default()
        };
        assert!(matches!(
            acceptance_rate(&c, Kind::SpecDefect),
            Measurement::Missing(_)
        ));
        let all_rejected = tally(&[
            e("b", Kind::FollowUp, Ruling::Rejected),
            e("b", Kind::FollowUp, Ruling::Rejected),
        ]);
        match acceptance_rate(&all_rejected[0], Kind::FollowUp) {
            Measurement::Observed(r) => assert_eq!(r, 0.0),
            other => panic!("all-rejected is a real zero, got {other:?}"),
        }
    }

    #[test]
    fn parse_reads_both_markers_in_document_order() {
        let r = "FINDINGS\n\nFINDING: clause 3 names a field that does not exist\nWHERE: line 12\n\nFOLLOWUP: swap Vec::contains for a set\nWHY: it gates deletion\n";
        let p = parse(r);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].kind, Kind::SpecDefect);
        assert_eq!(p[0].title, "clause 3 names a field that does not exist");
        assert_eq!(p[0].body, "WHERE: line 12");
        assert_eq!(p[1].kind, Kind::FollowUp);
        assert_eq!(p[1].body, "WHY: it gates deletion");
    }

    /// Four bugs in this project came from a scanner matching its own syntax
    /// quoted in prose, and the prompts that carry these markers name them.
    #[test]
    fn a_marker_mid_sentence_is_not_a_proposal() {
        let p = parse("write one FINDING: like this\n  FOLLOWUP: indented counts\n");
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].title, "indented counts");
    }

    #[test]
    fn a_marker_with_no_title_is_skipped_and_does_not_swallow_the_next() {
        let p = parse("FINDING:   \nsome orphaned body\nFOLLOWUP: real one\n");
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].title, "real one");
        assert_eq!(p[0].kind, Kind::FollowUp);
    }

    #[test]
    fn a_report_with_no_markers_yields_nothing() {
        assert_eq!(parse("NO SPEC DEFECTS FOUND.\n"), vec![]);
        assert_eq!(parse(""), vec![]);
    }

    /// An arm cannot raise its count by splitting one finding into five: the
    /// adjudicator rules the splits Duplicate, and the rate is unharmed while
    /// the accepted count does not inflate.
    #[test]
    fn splitting_one_finding_does_not_inflate_the_accepted_count() {
        let t = tally(&[
            e("a", Kind::SpecDefect, Ruling::Accepted),
            e("a", Kind::SpecDefect, Ruling::Duplicate),
            e("a", Kind::SpecDefect, Ruling::Duplicate),
        ]);
        assert_eq!(t[0].spec_accepted, 1);
        assert_eq!(t[0].spec_duplicate, 2);
    }

    fn followup(body: &str) -> Proposal {
        Proposal {
            kind: Kind::FollowUp,
            title: "swap the comparison".to_string(),
            body: body.to_string(),
        }
    }

    /// Evidence separates the paths the author already owns from the rest. It does NOT
    /// say what to do about them: a one-line change in a foreign file is trivial and a
    /// redesign in the author's own file is a week, and this struct knows neither.
    #[test]
    fn evidence_partitions_scope_without_ruling_on_it() {
        let p = followup("SCOPE: crates/farmerbob-core/src/cost.rs, crates/fb/src/pareto.rs");
        let declared = vec!["crates/farmerbob-core/src/cost.rs".to_string()];
        let e = evidence(&p, &declared, true);
        assert_eq!(
            e.inside,
            vec!["crates/farmerbob-core/src/cost.rs".to_string()]
        );
        assert_eq!(e.outside, vec!["crates/fb/src/pareto.rs".to_string()]);
        assert_eq!(e.scope.len(), 2);
    }

    /// A follow-up with no SCOPE names nothing. This is the one case a judge can dispose
    /// of without reading further, and it is still not a rejection.
    #[test]
    fn a_followup_without_a_scope_gathers_an_empty_scope() {
        let p = followup("WHY: it is backwards and costs a wrong frontier");
        let e = evidence(&p, &["crates/x.rs".to_string()], true);
        assert!(e.scope.is_empty());
        assert!(e.inside.is_empty());
        assert!(e.outside.is_empty());
    }

    /// Without the author's worktree an in-turn ruling is unavailable whatever the work
    /// costs -- there is nothing to resume into. Same follow-up, one flag different.
    #[test]
    fn a_missing_author_worktree_is_recorded_as_evidence() {
        let p = followup("SCOPE: crates/farmerbob-core/src/cost.rs");
        let declared = vec!["crates/farmerbob-core/src/cost.rs".to_string()];
        assert!(evidence(&p, &declared, true).author_worktree);
        assert!(!evidence(&p, &declared, false).author_worktree);
    }

    /// Paths are compared exactly: a leading `./` is a different string and is not guessed
    /// equivalent, because guessing is how a check silently permits something.
    #[test]
    fn scope_paths_are_compared_exactly() {
        let p = followup("SCOPE: ./crates/farmerbob-core/src/cost.rs");
        let declared = vec!["crates/farmerbob-core/src/cost.rs".to_string()];
        assert!(evidence(&p, &declared, true).inside.is_empty());
    }

    /// The three rulings are distinct values. There is deliberately no function from
    /// Evidence to Route: if one existed, this module would be deciding again.
    #[test]
    fn route_is_a_ruling_with_no_derivation() {
        assert_ne!(Route::InTurn, Route::Task);
        assert_ne!(Route::Task, Route::Unroutable);
    }
}
