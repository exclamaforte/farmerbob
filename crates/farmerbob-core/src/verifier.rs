//! Verifier trust: whose labels may reach a posterior, and which must be retracted.
//!
//! Implementers and verifiers fail differently. A bad implementer produces a bad
//! artefact; a bad verifier produces a bad *label* about an artefact that may be
//! fine. Because labels flow into every posterior that consumed them, a wrong
//! label is worse than a missing one: it must be possible to name exactly which
//! labels to withdraw once a verifier is found faulty.
//!
//! This module is therefore separate from the implementer routing machinery. A
//! [`Registry`] records every [`Label`] a verifier produced, tracks each
//! verifier's [`Trust`] independently of any implementer reputation, and answers
//! two questions: which labels are safe to consume ([`Registry::usable`]) and
//! which must be withdrawn ([`Registry::retracted`]). The free functions reason
//! about the instrument itself: [`suspicious_unanimity`] flags tasks where
//! unanimous agreement is evidence about the verifiers rather than the
//! candidates, and [`agreement`] measures how often one verifier sides with the
//! majority.
//!
//! Pure logic: no I/O, no clock, no randomness.

use std::collections::{BTreeMap, BTreeSet};

/// A label a verifier produced about one candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    /// Identity of the verifier that produced this label.
    pub verifier: String,
    /// Identity of the candidate that was judged.
    pub candidate: String,
    /// Identity of the task the candidate addresses.
    pub task: String,
    /// True when the verifier judged the candidate correct.
    pub passed: bool,
    /// Sequence number, so retraction can be ordered. Unique per verifier.
    pub seq: u64,
}

/// Trust earned by one verifier, tracked separately from any implementer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// Never evaluated. Its labels are recorded but must not reach a posterior.
    Untrusted,
    /// Agrees with the consensus often enough to be believed.
    Trusted,
    /// Demonstrated faulty. Its labels are retracted from `since` onward.
    Faulty {
        /// First sequence number that can no longer be believed.
        since: u64,
    },
}

/// Internal trust state stored per verifier.
///
/// Absence from the map means [`Trust::Untrusted`]; only earned states are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// Earned belief through agreement with the consensus.
    Trusted,
    /// Demonstrated faulty from a sequence number onward.
    Faulty {
        /// First sequence number that can no longer be believed.
        since: u64,
    },
}

/// Records verifier labels and the trust each verifier has earned.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registry {
    /// Every accepted label, in arrival order.
    labels: Vec<Label>,
    /// Earned standing per verifier; missing means unproven.
    standing: BTreeMap<String, Standing>,
}

impl Registry {
    /// Create an empty registry with no labels and no earned trust.
    pub fn new() -> Self {
        Self {
            labels: Vec::new(),
            standing: BTreeMap::new(),
        }
    }

    /// Record a label.
    ///
    /// Recording the same `(verifier, seq)` twice keeps the first and ignores
    /// the second, so a retried submission cannot rewrite history.
    pub fn record(&mut self, l: Label) {
        for existing in &self.labels {
            if existing.verifier == l.verifier && existing.seq == l.seq {
                return;
            }
        }
        self.labels.push(l);
    }

    /// Promote a verifier to [`Trust::Trusted`].
    ///
    /// Call this after `n` of the verifier's labels agreed with the consensus;
    /// the counting itself is the caller's job, `n` is only the policy record
    /// of how much agreement earned the promotion. Errors if the verifier is
    /// already [`Trust::Faulty`]: exoneration is a human decision, not an
    /// automatic one, so the state is left unchanged.
    pub fn promote(&mut self, verifier: &str, n: u32) -> Result<(), String> {
        let _ = n;
        match self.standing.get(verifier) {
            Some(Standing::Faulty { since }) => {
                let since = *since;
                Err(format!(
                    "verifier '{verifier}' is faulty since {since}; exoneration is a human decision"
                ))
            }
            _ => {
                self.standing
                    .insert(verifier.to_string(), Standing::Trusted);
                Ok(())
            }
        }
    }

    /// Mark a verifier faulty from `since` onward.
    ///
    /// Idempotent; an earlier `since` always wins, because the first evidence
    /// of a fault bounds what can still be believed.
    pub fn mark_faulty(&mut self, verifier: &str, since: u64) {
        match self.standing.get(verifier) {
            Some(Standing::Faulty { since: prev }) => {
                let prev = *prev;
                if since < prev {
                    self.standing
                        .insert(verifier.to_string(), Standing::Faulty { since });
                }
            }
            _ => {
                self.standing
                    .insert(verifier.to_string(), Standing::Faulty { since });
            }
        }
    }

    /// Current trust of a verifier; [`Trust::Untrusted`] when never evaluated.
    pub fn trust(&self, verifier: &str) -> Trust {
        match self.standing.get(verifier) {
            Some(Standing::Trusted) => Trust::Trusted,
            Some(Standing::Faulty { since }) => Trust::Faulty { since: *since },
            None => Trust::Untrusted,
        }
    }

    /// Labels that must be withdrawn from every posterior that consumed them.
    ///
    /// Only labels with `seq >= since` from [`Trust::Faulty`] verifiers are
    /// returned. Ordered by verifier then seq.
    pub fn retracted(&self) -> Vec<&Label> {
        let mut out: Vec<&Label> = Vec::new();
        for label in &self.labels {
            if let Some(Standing::Faulty { since }) = self.standing.get(label.verifier.as_str())
                && label.seq >= *since
            {
                out.push(label);
            }
        }
        out.sort_by(|a, b| a.verifier.cmp(&b.verifier).then_with(|| a.seq.cmp(&b.seq)));
        out
    }

    /// Labels safe to consume: from a Trusted verifier, and not retracted.
    ///
    /// Labels from [`Trust::Untrusted`] verifiers are unproven rather than
    /// faulty, so they appear in neither [`Registry::retracted`] nor here.
    /// Labels from [`Trust::Faulty`] verifiers never qualify, whether or not
    /// they fall inside the retraction window.
    pub fn usable(&self) -> Vec<&Label> {
        let mut out: Vec<&Label> = Vec::new();
        for label in &self.labels {
            if matches!(
                self.standing.get(label.verifier.as_str()),
                Some(Standing::Trusted)
            ) {
                out.push(label);
            }
        }
        out
    }
}

/// Tasks whose unanimous agreement indicts the instrument, not the candidates.
///
/// Returns the tasks where every recorded label agrees on `passed` and at
/// least `min_verifiers` distinct verifiers contributed, sorted by task id.
/// These are the tasks to re-check first: unanimity across independent
/// verifiers is evidence that the check itself is broken.
pub fn suspicious_unanimity(r: &Registry, min_verifiers: usize) -> Vec<String> {
    let mut per_task: BTreeMap<String, (BTreeSet<String>, bool, bool)> = BTreeMap::new();
    for label in &r.labels {
        let entry = per_task.entry(label.task.clone());
        let (verifiers, seen_pass, seen_fail) =
            entry.or_insert_with(|| (BTreeSet::new(), false, false));
        verifiers.insert(label.verifier.clone());
        if label.passed {
            *seen_pass = true;
        } else {
            *seen_fail = true;
        }
    }
    let mut out: Vec<String> = Vec::new();
    for (task, (verifiers, seen_pass, seen_fail)) in &per_task {
        let agreed = *seen_pass != *seen_fail;
        if agreed && verifiers.len() >= min_verifiers {
            out.push(task.clone());
        }
    }
    out
}

/// Fraction of a verifier's labels that match the majority for their pair.
///
/// Each of the verifier's labels is compared against the majority `passed`
/// over all labels for the same `(task, candidate)`. Only pairs labelled by
/// at least two distinct verifiers count; a pair only this verifier judged
/// says nothing about it. An exact tie counts as agreement for every
/// verifier, since no majority exists to disagree with.
///
/// Returns `None` when the verifier has no label with a comparable peer,
/// including when it has no labels at all.
pub fn agreement(r: &Registry, verifier: &str) -> Option<f64> {
    let mut per_pair: BTreeMap<(String, String), (usize, usize, BTreeSet<String>)> =
        BTreeMap::new();
    for label in &r.labels {
        let key = (label.task.clone(), label.candidate.clone());
        let entry = per_pair.entry(key);
        let (passes, fails, verifiers) = entry.or_insert_with(|| (0usize, 0usize, BTreeSet::new()));
        if label.passed {
            *passes += 1;
        } else {
            *fails += 1;
        }
        verifiers.insert(label.verifier.clone());
    }
    let mut matched: usize = 0;
    let mut total: usize = 0;
    for label in &r.labels {
        if label.verifier.as_str() != verifier {
            continue;
        }
        let key = (label.task.clone(), label.candidate.clone());
        let Some((passes, fails, verifiers)) = per_pair.get(&key) else {
            continue;
        };
        if verifiers.len() < 2 {
            continue;
        }
        total += 1;
        if passes == fails {
            matched += 1;
        } else if *passes > *fails {
            if label.passed {
                matched += 1;
            }
        } else if !label.passed {
            matched += 1;
        }
    }
    if total == 0 {
        None
    } else {
        Some(matched as f64 / total as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(verifier: &str, candidate: &str, task: &str, passed: bool, seq: u64) -> Label {
        Label {
            verifier: verifier.to_string(),
            candidate: candidate.to_string(),
            task: task.to_string(),
            passed,
            seq,
        }
    }

    #[test]
    fn untrusted_labels_are_neither_usable_nor_retracted() {
        let mut r = Registry::new();
        r.record(label("v1", "c1", "t1", true, 0));
        assert_eq!(r.trust("v1"), Trust::Untrusted);
        assert!(r.usable().is_empty());
        assert!(r.retracted().is_empty());
    }

    #[test]
    fn earliest_since_wins_across_repeated_mark_faulty() {
        let mut r = Registry::new();
        r.record(label("v1", "c1", "t1", true, 5));
        r.record(label("v1", "c2", "t1", true, 9));
        r.mark_faulty("v1", 9);
        r.mark_faulty("v1", 5);
        assert_eq!(r.trust("v1"), Trust::Faulty { since: 5 });
        // The later call with a later `since` must not narrow the window.
        r.mark_faulty("v1", 12);
        assert_eq!(r.trust("v1"), Trust::Faulty { since: 5 });
        assert_eq!(r.retracted().len(), 2);
    }

    #[test]
    fn promote_on_faulty_errors_and_changes_nothing() {
        let mut r = Registry::new();
        r.record(label("v1", "c1", "t1", true, 0));
        r.mark_faulty("v1", 0);
        let before = r.clone();
        let result = r.promote("v1", 3);
        assert!(result.is_err());
        assert_eq!(r, before);
        assert_eq!(r.trust("v1"), Trust::Faulty { since: 0 });
    }

    #[test]
    fn label_below_since_survives_retraction() {
        let mut r = Registry::new();
        r.record(label("v1", "c1", "t1", true, 1));
        r.record(label("v1", "c2", "t1", false, 7));
        r.mark_faulty("v1", 5);
        let retracted = r.retracted();
        assert_eq!(retracted.len(), 1);
        assert_eq!(retracted[0].seq, 7);
        // The faulty verifier's labels are never usable, but the old one must
        // not be reported as retracted either.
        assert!(r.usable().is_empty());
    }

    #[test]
    fn agreement_ignores_singleton_pairs() {
        let mut r = Registry::new();
        // v1 alone on (t1, c1): no peer, must not count.
        r.record(label("v1", "c1", "t1", true, 0));
        assert_eq!(agreement(&r, "v1"), None);
        // Add a peer on a different pair where they agree.
        r.record(label("v1", "c2", "t2", true, 1));
        r.record(label("v2", "c2", "t2", true, 0));
        assert_eq!(agreement(&r, "v1"), Some(1.0));
    }

    #[test]
    fn tie_counts_as_agreement_for_every_verifier() {
        let mut r = Registry::new();
        r.record(label("v1", "c1", "t1", true, 0));
        r.record(label("v2", "c1", "t1", false, 0));
        assert_eq!(agreement(&r, "v1"), Some(1.0));
        assert_eq!(agreement(&r, "v2"), Some(1.0));
    }

    #[test]
    fn unanimity_below_min_verifiers_is_not_reported() {
        let mut r = Registry::new();
        r.record(label("v1", "c1", "t1", true, 0));
        r.record(label("v2", "c1", "t1", true, 0));
        assert!(suspicious_unanimity(&r, 3).is_empty());
        assert_eq!(suspicious_unanimity(&r, 2), vec!["t1".to_string()]);
    }

    #[test]
    fn duplicate_seq_keeps_the_first_and_ignores_the_second() {
        let mut r = Registry::new();
        r.record(label("v1", "c1", "t1", true, 0));
        r.record(label("v1", "c9", "t9", false, 0));
        r.promote("v1", 1).unwrap_or(());
        let usable = r.usable();
        assert_eq!(usable.len(), 1);
        assert_eq!(usable[0].candidate, "c1");
        assert_eq!(usable[0].task, "t1");
        assert!(usable[0].passed);
    }

    // --- Extra coverage beyond the one-per-rule minimum ---

    #[test]
    fn promote_moves_untrusted_to_trusted_and_labels_become_usable() {
        let mut r = Registry::new();
        r.record(label("v1", "c1", "t1", true, 0));
        assert!(r.usable().is_empty());
        assert!(r.promote("v1", 1).is_ok());
        assert_eq!(r.trust("v1"), Trust::Trusted);
        assert_eq!(r.usable().len(), 1);
        assert!(r.retracted().is_empty());
    }

    #[test]
    fn retraction_window_includes_seq_equal_to_since_and_is_ordered() {
        let mut r = Registry::new();
        r.record(label("vb", "c1", "t1", true, 3));
        r.record(label("va", "c1", "t1", true, 5));
        r.record(label("va", "c2", "t1", true, 2));
        r.mark_faulty("va", 2);
        r.mark_faulty("vb", 3);
        let retracted = r.retracted();
        let keys: Vec<(&str, u64)> = retracted
            .iter()
            .map(|l| (l.verifier.as_str(), l.seq))
            .collect();
        assert_eq!(keys, vec![("va", 2), ("va", 5), ("vb", 3)]);
    }

    #[test]
    fn unknown_verifier_is_untrusted_with_no_agreement() {
        let r = Registry::new();
        assert_eq!(r.trust("ghost"), Trust::Untrusted);
        assert_eq!(agreement(&r, "ghost"), None);
        assert!(r.usable().is_empty());
        assert!(r.retracted().is_empty());
    }

    #[test]
    fn agreement_measures_majority_fraction() {
        let mut r = Registry::new();
        // (t1,c1): v1 + v2 pass, v3 fails -> majority pass.
        r.record(label("v1", "c1", "t1", true, 0));
        r.record(label("v2", "c1", "t1", true, 0));
        r.record(label("v3", "c1", "t1", false, 0));
        // (t1,c2): v1 fails, v2 + v3 pass -> majority pass, v1 disagrees.
        r.record(label("v1", "c2", "t1", false, 1));
        r.record(label("v2", "c2", "t1", true, 1));
        r.record(label("v3", "c2", "t1", true, 1));
        assert_eq!(agreement(&r, "v1"), Some(0.5));
        assert_eq!(agreement(&r, "v2"), Some(1.0));
        assert_eq!(agreement(&r, "v3"), Some(0.5));
    }

    #[test]
    fn unanimity_requires_agreement_and_reports_sorted_tasks() {
        let mut r = Registry::new();
        r.record(label("v1", "c1", "tb", true, 0));
        r.record(label("v2", "c1", "tb", true, 0));
        r.record(label("v1", "c1", "ta", false, 1));
        r.record(label("v2", "c1", "ta", false, 1));
        // Disagreeing task is never suspicious.
        r.record(label("v1", "c1", "tc", true, 2));
        r.record(label("v2", "c1", "tc", false, 2));
        assert_eq!(
            suspicious_unanimity(&r, 2),
            vec!["ta".to_string(), "tb".to_string()]
        );
    }

    #[test]
    fn faulty_labels_leave_usable_even_inside_the_window() {
        let mut r = Registry::new();
        r.record(label("v1", "c1", "t1", true, 0));
        assert!(r.promote("v1", 1).is_ok());
        assert_eq!(r.usable().len(), 1);
        r.mark_faulty("v1", 0);
        assert!(r.usable().is_empty());
        assert_eq!(r.retracted().len(), 1);
    }
}
