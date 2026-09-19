//! Who reviews whom, and what each reviewer is shown.
//!
//! Before a critique spends a single token, two pure decisions are made.
//! Assignment: arm `i` reviews arm `i+1` round the ring — a derangement, so
//! nobody reviews themselves. Presentation: the critic is shown the
//! deliverable, not "whatever git happens to call a change".
//!
//! The presentation rule exists because it was once wrong. The script ran
//! `git diff HEAD -- crates/$CRATE` and fell back to reading the file only
//! when the diff came back EMPTY. For a task that CREATES its target the
//! deliverable is untracked, so the diff held exactly one tracked line,
//! `+pub mod matrix;` — not empty, so the fallback never fired, and the
//! critic reviewed a module declaration as if it were the work. The guard
//! was defeated by the very line that made the patch useless.
//! (bead farmerbob-4ur)
//!
//! The distinction that matters is therefore not empty-versus-absent but
//! worth-showing-versus-not: `Some("")` and `None` for the diff take the
//! same branch, while a one-line diff of the declared target is a real diff
//! and is shown as one.
//!
//! This module decides; it does not read worktrees. Gathering diffs,
//! contents, and strayed-path lists is the caller's job.

/// One arm that produced something reviewable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Arm name, e.g. "glm-53-flash".
    pub arm: String,
    /// The diff of the declared target in this arm's worktree, if the target
    /// is TRACKED. `None` and `Some("")` are different values that mean the
    /// same thing to [`patch_for`]: neither is a diff worth showing.
    pub target_diff: Option<String>,
    /// The full contents of the declared target on disk, if it exists at
    /// all. An empty file is not a deliverable.
    pub target_contents: Option<String>,
    /// Paths this arm changed that are NOT the declared target and NOT the
    /// module declaration line, in the order given. Taken as given: this
    /// module neither computes nor filters the list.
    pub strayed: Vec<String>,
}

/// What one critic is asked to review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    /// The arm doing the reviewing.
    pub critic: String,
    /// The arm being reviewed.
    pub subject: String,
    /// The artefact the critic is shown: the subject's deliverable.
    pub patch: Patch,
    /// The subject's [`Candidate::strayed`], unchanged, same order — what
    /// else the subject touched besides the target.
    pub strayed: Vec<String>,
}

/// The artefact a critic is shown. Exactly two variants: a caller puts one
/// artefact in front of one critic, so there is no third state between "the
/// diff" and "the file".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Patch {
    /// The target is tracked and has a non-empty, non-whitespace diff.
    /// Carries that diff verbatim.
    Diff(String),
    /// The target is untracked or unchanged, and exists on disk. Carries its
    /// contents and its line count.
    NewFile {
        /// The file's full contents, verbatim.
        contents: String,
        /// The number of lines in `contents`.
        lines: usize,
    },
}

/// Why no plan could be made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoPlan {
    /// Fewer than two candidates have something reviewable. One arm cannot
    /// review itself, and there is nobody else to review it.
    TooFew {
        /// How many candidates had something to show.
        reviewable: usize,
    },
}

/// What a single candidate would be shown as, or `None` if it has nothing
/// to show.
///
/// The diff is shown when it is present, non-empty, and not entirely
/// whitespace; absent, empty, and whitespace-only all fall through to the
/// same next test, the contents on disk. `Some("")` and `None` for
/// `target_diff` reach the same branch. A short diff of the declared target
/// — one line among them — is still a real diff.
pub fn patch_for(c: &Candidate) -> Option<Patch> {
    if let Some(diff) = c.target_diff.as_deref()
        && !diff.trim().is_empty()
    {
        return Some(Patch::Diff(diff.to_string()));
    }
    match c.target_contents.as_deref() {
        Some(contents) if !contents.is_empty() => Some(Patch::NewFile {
            contents: contents.to_string(),
            lines: contents.lines().count(),
        }),
        _ => None,
    }
}

/// Assign every reviewable candidate exactly one subject: arm `i` reviews
/// arm `i+1` round the ring, wrapping at the end. The wrap-around is a
/// derangement — nobody reviews themselves — and at two reviewable arms the
/// forced swap is its own inverse.
///
/// Candidates with nothing to show ([`patch_for`] returns `None`) are
/// dropped BEFORE the assignment is computed, so the ring runs over the
/// reviewable set, not over the input; an unreviewable arm appears nowhere
/// in the plan. The returned vector is in critic order, which follows the
/// order of the reviewable candidates in the input.
///
/// Fails with [`NoPlan::TooFew`] when fewer than two candidates are
/// reviewable.
pub fn plan(candidates: &[Candidate]) -> Result<Vec<Assignment>, NoPlan> {
    let reviewable: Vec<(&Candidate, Patch)> = candidates
        .iter()
        .filter_map(|c| patch_for(c).map(|patch| (c, patch)))
        .collect();
    if reviewable.len() < 2 {
        return Err(NoPlan::TooFew {
            reviewable: reviewable.len(),
        });
    }
    Ok(reviewable
        .iter()
        .zip(reviewable.iter().skip(1).chain(reviewable.iter().take(1)))
        .map(|((critic, _), (subject, patch))| Assignment {
            critic: critic.arm.clone(),
            subject: subject.arm.clone(),
            patch: patch.clone(),
            strayed: subject.strayed.clone(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(arm: &str, diff: Option<&str>, contents: Option<&str>) -> Candidate {
        Candidate {
            arm: arm.to_string(),
            target_diff: diff.map(str::to_string),
            target_contents: contents.map(str::to_string),
            strayed: Vec::new(),
        }
    }

    fn planned(candidates: &[Candidate]) -> Vec<Assignment> {
        match plan(candidates) {
            Ok(assignments) => assignments,
            Err(_) => panic!("expected a plan"),
        }
    }

    #[test]
    fn non_empty_diff_is_shown_verbatim() {
        let diff = "+pub mod matrix;\n-pub mod old;\n";
        match patch_for(&candidate("arm", Some(diff), None)) {
            Some(Patch::Diff(carried)) => assert_eq!(carried, diff),
            _ => panic!("expected Diff"),
        }
    }

    #[test]
    fn one_line_module_declaration_diff_is_a_real_diff() {
        let c = candidate("arm", Some("+pub mod matrix;\n"), Some("pub mod matrix;\n"));
        match patch_for(&c) {
            Some(Patch::Diff(carried)) => assert_eq!(carried, "+pub mod matrix;\n"),
            _ => panic!("expected Diff, not NewFile"),
        }
    }

    #[test]
    fn empty_and_absent_diff_take_the_same_branch() {
        let contents = "fn main() {}\n";
        let empty_diff = patch_for(&candidate("a", Some(""), Some(contents)));
        let absent_diff = patch_for(&candidate("b", None, Some(contents)));
        match (empty_diff, absent_diff) {
            (Some(Patch::NewFile { .. }), Some(Patch::NewFile { .. })) => {}
            _ => panic!("Some(\"\") and None must both reach NewFile"),
        }
    }

    #[test]
    fn whitespace_only_diff_falls_through_to_contents() {
        match patch_for(&candidate("a", Some("  \n\t"), Some("body"))) {
            Some(Patch::NewFile { contents, lines }) => {
                assert_eq!(contents, "body");
                assert_eq!(lines, 1);
            }
            _ => panic!("expected NewFile"),
        }
    }

    #[test]
    fn no_diff_worth_showing_and_no_contents_is_not_reviewable() {
        assert!(patch_for(&candidate("a", None, None)).is_none());
        assert!(patch_for(&candidate("a", Some(""), None)).is_none());
        assert!(patch_for(&candidate("a", Some(" \n"), None)).is_none());
    }

    #[test]
    fn empty_contents_are_not_a_deliverable() {
        assert!(patch_for(&candidate("a", None, Some(""))).is_none());
        assert!(patch_for(&candidate("a", Some(""), Some(""))).is_none());
    }

    #[test]
    fn new_file_carries_contents_and_line_count() {
        match patch_for(&candidate("a", None, Some("fn a() {}\nfn b() {}"))) {
            Some(Patch::NewFile { contents, lines }) => {
                assert_eq!(contents, "fn a() {}\nfn b() {}");
                assert_eq!(lines, 2);
            }
            _ => panic!("expected NewFile"),
        }
    }

    #[test]
    fn zero_candidates_is_too_few() {
        match plan(&[]) {
            Err(NoPlan::TooFew { reviewable }) => assert_eq!(reviewable, 0),
            Ok(_) => panic!("expected TooFew"),
        }
    }

    #[test]
    fn one_reviewable_arm_is_too_few() {
        let solo = [candidate("a", Some("diff"), None)];
        match plan(&solo) {
            Err(NoPlan::TooFew { reviewable }) => assert_eq!(reviewable, 1),
            Ok(_) => panic!("expected TooFew"),
        }
    }

    #[test]
    fn two_unreviewable_arms_report_zero() {
        let pair = [candidate("a", None, None), candidate("b", None, None)];
        match plan(&pair) {
            Err(NoPlan::TooFew { reviewable }) => assert_eq!(reviewable, 0),
            Ok(_) => panic!("expected TooFew"),
        }
    }

    #[test]
    fn two_reviewable_arms_review_each_other() {
        let pair = [
            candidate("a", Some("a-diff"), None),
            candidate("b", None, Some("b-contents")),
        ];
        let plan = planned(&pair);
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].critic, "a");
        assert_eq!(plan[0].subject, "b");
        assert_eq!(plan[1].critic, "b");
        assert_eq!(plan[1].subject, "a");
        match &plan[0].patch {
            Patch::NewFile { contents, .. } => assert_eq!(contents, "b-contents"),
            _ => panic!("expected the subject's contents, not the critic's diff"),
        }
    }

    #[test]
    fn three_reviewable_arms_form_a_three_cycle() {
        let trio = [
            candidate("a", Some("1"), None),
            candidate("b", Some("2"), None),
            candidate("c", Some("3"), None),
        ];
        let plan = planned(&trio);
        let reviewed: Vec<(&str, &str)> = plan
            .iter()
            .map(|a| (a.critic.as_str(), a.subject.as_str()))
            .collect();
        assert_eq!(reviewed, vec![("a", "b"), ("b", "c"), ("c", "a")]);
    }

    #[test]
    fn four_reviewable_arms_nobody_reviews_themselves() {
        let quartet: Vec<Candidate> = ["a", "b", "c", "d"]
            .iter()
            .map(|arm| candidate(arm, Some("d"), None))
            .collect();
        let plan = planned(&quartet);
        assert_eq!(plan.len(), 4);
        let mut critics: Vec<&str> = Vec::new();
        let mut subjects: Vec<&str> = Vec::new();
        for a in &plan {
            assert_ne!(a.critic, a.subject);
            critics.push(a.critic.as_str());
            subjects.push(a.subject.as_str());
        }
        critics.sort_unstable();
        subjects.sort_unstable();
        assert_eq!(critics, vec!["a", "b", "c", "d"]);
        assert_eq!(subjects, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn unreviewable_arm_appears_nowhere_in_the_plan() {
        let trio = [
            candidate("a", Some("1"), None),
            candidate("gone", None, None),
            candidate("c", Some("3"), None),
        ];
        let plan = planned(&trio);
        assert_eq!(plan.len(), 2);
        let reviewed: Vec<(&str, &str)> = plan
            .iter()
            .map(|a| (a.critic.as_str(), a.subject.as_str()))
            .collect();
        assert_eq!(reviewed, vec![("a", "c"), ("c", "a")]);
    }

    #[test]
    fn order_follows_the_reviewable_candidates() {
        let trio = [
            candidate("gone", None, None),
            candidate("b", Some("2"), None),
            candidate("c", Some("3"), None),
        ];
        let plan = planned(&trio);
        let critics: Vec<&str> = plan.iter().map(|a| a.critic.as_str()).collect();
        assert_eq!(critics, vec!["b", "c"]);
    }

    #[test]
    fn strayed_is_the_subjects_list_in_order() {
        let mut a = candidate("a", Some("1"), None);
        a.strayed = vec!["a-stray-1".into(), "a-stray-2".into()];
        let mut b = candidate("b", Some("2"), None);
        b.strayed = vec!["b-stray".into()];
        let plan = planned(&[a, b]);
        assert_eq!(plan[0].strayed, vec!["b-stray".to_string()]);
        assert_eq!(
            plan[1].strayed,
            vec!["a-stray-1".to_string(), "a-stray-2".to_string()]
        );
    }
}
