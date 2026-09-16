//! A defect corpus harvested from the orchestrator's own history.
//!
//! Synthetic mutations are weak: a hand-written mutant tends to break
//! something every suite already checks. The defects worth scoring against
//! are the ones that actually happened — a scoring gate that passed an empty
//! suite, a confinement wrapper silently dropped in a refactor — so every
//! entry here carries two things: the evidence that it really occurred, and
//! the assertion that would have caught it. An entry that cannot state both
//! is refused at the door, never stored.
//!
//! The admission rule that shapes the arithmetic: a defect is admitted only
//! if some suite detects it. A mutation nothing detects is inert, or the
//! specification is silent about it; neither is a miss on any candidate's
//! part, so both leave the denominator rather than counting against everyone
//! equally. The same care governs duplicates, which inflate a denominator
//! quietly: two entries are the same defect when they name the same module
//! and the same detecting assertion, whatever their ids or wording, and only
//! the first admission is kept.
//!
//! Pure logic: no I/O.

use std::collections::BTreeMap;

/// How a defect was found, in descending order of how much it says about
/// tests.
///
/// The derived ordering runs [`FoundBy::Test`] < [`FoundBy::CrossExamination`]
/// < [`FoundBy::Review`] < [`FoundBy::Observation`]; [`Corpus::histogram`] is
/// ordered by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FoundBy {
    /// A test failed. The cheapest possible discovery.
    Test,
    /// Cross-examination: another candidate's suite caught it.
    CrossExamination,
    /// A reviewer reading the patch.
    Review,
    /// Only noticed in production behaviour. The most expensive, and the most
    /// valuable to add a test for.
    Observation,
}

/// One real defect, with the proof it happened and the check that would have
/// caught it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Defect {
    /// Name of this entry. Identifies the entry, but takes no part in defect
    /// identity: see [`same_defect`].
    pub id: String,
    /// What went wrong, in one line.
    pub summary: String,
    /// Where it lived.
    pub module: String,
    /// How it was discovered.
    pub found_by: FoundBy,
    /// The assertion that would have caught it. Empty means the entry is not
    /// admissible.
    pub detecting_assertion: String,
    /// Evidence it really happened: a commit, a bead id, a log line.
    pub evidence: String,
}

/// Why an entry was refused admission. A refused entry is never stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inadmissible {
    /// No assertion would catch it: inert, or the spec is silent.
    NoDetectingAssertion,
    /// No evidence it occurred. An invented defect is a mutation, not a
    /// harvest.
    NoEvidence,
    /// The same defect is already in the corpus; `of` names the entry kept.
    Duplicate {
        /// The id of the admitted defect this one duplicates.
        of: String,
    },
}

/// The harvested corpus: only defects some suite detects, none of them
/// invented, none of them counted twice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Corpus {
    /// Admitted defects, in admission order; the readers below sort.
    defects: Vec<Defect>,
}

impl Corpus {
    /// An empty corpus.
    pub fn new() -> Self {
        Self::default()
    }

    /// Admit `d`, or refuse it with the reason. A refused entry is never
    /// stored.
    ///
    /// A missing detecting assertion is checked before missing evidence when
    /// both are absent. A defect that [`same_defect`] holds against any
    /// admitted defect is refused as a duplicate: the first admission is the
    /// one kept, and the refusal names its id.
    pub fn admit(&mut self, d: Defect) -> Result<(), Inadmissible> {
        if d.detecting_assertion.trim().is_empty() {
            return Err(Inadmissible::NoDetectingAssertion);
        }
        if d.evidence.trim().is_empty() {
            return Err(Inadmissible::NoEvidence);
        }
        if let Some(kept) = self.defects.iter().find(|kept| same_defect(kept, &d)) {
            return Err(Inadmissible::Duplicate {
                of: kept.id.clone(),
            });
        }
        self.defects.push(d);
        Ok(())
    }

    /// Admitted defects, ordered by id, stable regardless of admission order.
    pub fn defects(&self) -> Vec<&Defect> {
        let mut refs: Vec<&Defect> = self.defects.iter().collect();
        refs.sort_by(|a, b| a.id.cmp(&b.id));
        refs
    }

    /// Defects whose module is exactly `module`, ordered by id.
    pub fn by_module(&self, module: &str) -> Vec<&Defect> {
        let mut refs: Vec<&Defect> = self.defects.iter().filter(|d| d.module == module).collect();
        refs.sort_by(|a, b| a.id.cmp(&b.id));
        refs
    }

    /// Count per discovery route: only routes that occur, in [`FoundBy`]
    /// order, the counts summing to [`Corpus::len`].
    ///
    /// The shape of this histogram says whether the test suites are carrying
    /// their weight or whether review is doing their job.
    pub fn histogram(&self) -> Vec<(FoundBy, u32)> {
        let (mut test, mut cross, mut review, mut observation) = (0u32, 0u32, 0u32, 0u32);
        for d in &self.defects {
            match d.found_by {
                FoundBy::Test => test += 1,
                FoundBy::CrossExamination => cross += 1,
                FoundBy::Review => review += 1,
                FoundBy::Observation => observation += 1,
            }
        }
        [
            (FoundBy::Test, test),
            (FoundBy::CrossExamination, cross),
            (FoundBy::Review, review),
            (FoundBy::Observation, observation),
        ]
        .into_iter()
        .filter(|&(_, n)| n > 0)
        .collect()
    }

    /// How many defects the corpus holds.
    pub fn len(&self) -> usize {
        self.defects.len()
    }

    /// Whether the corpus holds no defects.
    pub fn is_empty(&self) -> bool {
        self.defects.is_empty()
    }
}

/// Two defects are the same when they name the same module and the same
/// detecting assertion, compared ignoring case and surrounding whitespace.
/// Ids, summaries, evidence and discovery route do not take part: whatever
/// their wording, the same assertion over the same module catches the same
/// defect, and counting it twice inflates the denominator.
pub fn same_defect(a: &Defect, b: &Defect) -> bool {
    defect_key(&a.module) == defect_key(&b.module)
        && defect_key(&a.detecting_assertion) == defect_key(&b.detecting_assertion)
}

/// Modules whose defects only [`FoundBy::Review`] or [`FoundBy::Observation`]
/// found — nothing tested them. This is the list of places to write tests
/// next, ranked by defect count descending, ties broken by module name so the
/// ranking is deterministic. A module with even one [`FoundBy::Test`] or
/// [`FoundBy::CrossExamination`] defect is omitted: some suite already
/// watches it.
pub fn untested_modules(c: &Corpus) -> Vec<(String, u32)> {
    // module -> (untested defect count, any defect found by a suite)
    let mut acc: BTreeMap<&str, (u32, bool)> = BTreeMap::new();
    for d in &c.defects {
        let entry = acc.entry(d.module.as_str()).or_insert((0, false));
        match d.found_by {
            FoundBy::Test | FoundBy::CrossExamination => entry.1 = true,
            FoundBy::Review | FoundBy::Observation => entry.0 += 1,
        }
    }
    let mut ranked: Vec<(String, u32)> = acc
        .into_iter()
        .filter(|&(_, (_, tested))| !tested)
        .map(|(module, (count, _))| (module.to_owned(), count))
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
}

/// Fold a field down to its defect-identity form: surrounding whitespace
/// stripped, case folded.
fn defect_key(s: &str) -> String {
    s.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A defect with the given identity; summary and evidence are derived
    /// from the id so each test only says what it is about.
    fn defect(id: &str, module: &str, found_by: FoundBy, assertion: &str) -> Defect {
        Defect {
            id: id.to_string(),
            summary: format!("{id}: something went wrong"),
            module: module.to_string(),
            found_by,
            detecting_assertion: assertion.to_string(),
            evidence: format!("commit {id}"),
        }
    }

    /// Admit every case, failing the test on any refusal.
    fn admitted(cases: &[(&str, &str, FoundBy, &str)]) -> Corpus {
        let mut c = Corpus::new();
        for (i, (id, module, found_by, assertion)) in cases.iter().enumerate() {
            let d = defect(&format!("{id}-{i}"), module, *found_by, assertion);
            assert_eq!(c.admit(d), Ok(()), "case {i} must be admitted");
        }
        c
    }

    fn ids(c: &Corpus) -> Vec<String> {
        c.defects().iter().map(|d| d.id.clone()).collect()
    }

    #[test]
    fn a_missing_assertion_is_rejected_before_missing_evidence() {
        let mut c = Corpus::new();
        // Both are missing; the assertion is what the refusal reports.
        let d = defect("d1", "router", FoundBy::Review, "");
        assert_eq!(c.admit(d), Err(Inadmissible::NoDetectingAssertion));
        assert!(c.is_empty(), "a refused entry is never stored");
    }

    #[test]
    fn a_whitespace_only_assertion_is_no_detecting_assertion() {
        let mut c = Corpus::new();
        let mut d = defect("d1", "router", FoundBy::Review, "   \t\n  ");
        d.evidence = "commit abc".to_string();
        assert_eq!(c.admit(d), Err(Inadmissible::NoDetectingAssertion));
        assert!(c.is_empty());
    }

    #[test]
    fn missing_evidence_is_rejected() {
        let mut c = Corpus::new();
        let mut d = defect("d1", "router", FoundBy::Review, "assert(x > 0)");
        d.evidence = String::new();
        assert_eq!(c.admit(d.clone()), Err(Inadmissible::NoEvidence));
        d.evidence = "   ".to_string();
        assert_eq!(c.admit(d), Err(Inadmissible::NoEvidence));
        assert!(c.is_empty());
    }

    #[test]
    fn a_well_formed_defect_is_admitted_and_stored() {
        let mut c = Corpus::new();
        let d = defect("d1", "router", FoundBy::Review, "assert(!queue.is_empty())");
        assert_eq!(c.admit(d.clone()), Ok(()));
        assert!(!c.is_empty());
        assert_eq!(c.len(), 1);
        assert_eq!(c.defects(), vec![&d]);
    }

    #[test]
    fn differently_worded_entries_for_one_assertion_are_duplicates() {
        let mut c = Corpus::new();
        let first = defect("d1", "router", FoundBy::Test, "assert(!queue.is_empty())");
        assert_eq!(c.admit(first.clone()), Ok(()));

        // Same defect, different everything the identity is allowed to ignore.
        let mut second = defect(
            "d2",
            "ROUTER",
            FoundBy::Review,
            "  ASSERT(!QUEUE.IS_EMPTY())  ",
        );
        second.summary = "a completely different telling of the same story".to_string();
        second.evidence = "a different commit".to_string();
        assert!(same_defect(&first, &second));
        assert_eq!(
            c.admit(second),
            Err(Inadmissible::Duplicate {
                of: "d1".to_string()
            })
        );
    }

    #[test]
    fn a_duplicate_keeps_the_first_and_the_rejection_names_it() {
        let mut c = Corpus::new();
        assert_eq!(
            c.admit(defect(
                "keep-me",
                "slots",
                FoundBy::Test,
                "assert(len <= MAX)"
            )),
            Ok(())
        );
        let dup = defect("drop-me", "SLOTS", FoundBy::Review, "  assert(LEN <= max) ");
        assert_eq!(
            c.admit(dup),
            Err(Inadmissible::Duplicate {
                of: "keep-me".to_string()
            })
        );
        assert_eq!(c.len(), 1);
        assert_eq!(ids(&c), vec!["keep-me".to_string()]);
    }

    #[test]
    fn same_defect_compares_module_and_assertion_ignoring_case_and_surrounding_whitespace() {
        let a = defect("a", "  Router ", FoundBy::Test, "  assert(x == 1)  ");
        let b = defect("b", "router", FoundBy::Review, "ASSERT(X == 1)");
        assert!(same_defect(&a, &b));
        // Only surrounding whitespace is ignored: internal differences stand.
        let inner = defect("c", "router", FoundBy::Test, "assert(x  == 1)");
        assert!(!same_defect(&a, &inner));
    }

    #[test]
    fn same_defect_ignores_id_summary_evidence_and_found_by() {
        let a = defect("a", "quota", FoundBy::Test, "assert(!blocked.is_empty())");
        let mut b = defect(
            "b",
            "quota",
            FoundBy::Observation,
            "assert(!blocked.is_empty())",
        );
        b.summary = "another story entirely".to_string();
        b.evidence = "log line 42".to_string();
        assert!(same_defect(&a, &b));
    }

    #[test]
    fn the_same_assertion_in_a_different_module_is_not_a_duplicate() {
        assert!(!same_defect(
            &defect("a", "quota", FoundBy::Test, "assert(x > 0)"),
            &defect("b", "slots", FoundBy::Test, "assert(x > 0)"),
        ));
    }

    #[test]
    fn a_different_assertion_in_the_same_module_is_not_a_duplicate() {
        assert!(!same_defect(
            &defect("a", "quota", FoundBy::Test, "assert(x > 0)"),
            &defect("b", "quota", FoundBy::Test, "assert(x >= 0)"),
        ));
    }

    #[test]
    fn the_histogram_sums_to_len() {
        let c = admitted(&[
            ("d", "quota", FoundBy::Test, "assert(q > 0)"),
            ("d", "quota", FoundBy::Test, "assert(q > 1)"),
            ("d", "quota", FoundBy::Test, "assert(q > 2)"),
            ("d", "slots", FoundBy::CrossExamination, "assert(s > 0)"),
            ("d", "slots", FoundBy::CrossExamination, "assert(s > 1)"),
            ("d", "slots", FoundBy::CrossExamination, "assert(s > 2)"),
            ("d", "slots", FoundBy::CrossExamination, "assert(s > 3)"),
            ("d", "router", FoundBy::Review, "assert(r > 0)"),
            ("d", "router", FoundBy::Review, "assert(r > 1)"),
            ("d", "proto", FoundBy::Observation, "assert(p > 0)"),
        ]);
        assert_eq!(c.len(), 10);
        let total: usize = c.histogram().iter().map(|(_, n)| *n as usize).sum();
        assert_eq!(total, c.len());
    }

    #[test]
    fn the_histogram_lists_only_routes_that_occur_in_foundby_order() {
        // The ordering the histogram is pledged to.
        assert!(FoundBy::Test < FoundBy::CrossExamination);
        assert!(FoundBy::CrossExamination < FoundBy::Review);
        assert!(FoundBy::Review < FoundBy::Observation);

        let empty = Corpus::new();
        assert_eq!(empty.histogram(), vec![]);

        // Admitted scrambled; Review never occurs and must be absent.
        let c = admitted(&[
            ("d", "proto", FoundBy::Observation, "assert(p == 1)"),
            ("d", "quota", FoundBy::Test, "assert(q == 1)"),
            ("d", "router", FoundBy::Observation, "assert(r == 1)"),
            ("d", "slots", FoundBy::CrossExamination, "assert(s == 1)"),
            ("d", "quota", FoundBy::Test, "assert(q == 2)"),
        ]);
        assert_eq!(
            c.histogram(),
            vec![
                (FoundBy::Test, 2),
                (FoundBy::CrossExamination, 1),
                (FoundBy::Observation, 2),
            ]
        );
    }

    #[test]
    fn a_single_route_histogram_holds_just_that_route() {
        let c = admitted(&[("d", "proto", FoundBy::Review, "assert(v == 1)")]);
        assert_eq!(c.histogram(), vec![(FoundBy::Review, 1)]);
    }

    #[test]
    fn a_module_with_any_tested_defect_is_absent_from_untested_modules() {
        let c = admitted(&[
            ("d", "only-test", FoundBy::Test, "assert(t == 1)"),
            (
                "d",
                "only-crossx",
                FoundBy::CrossExamination,
                "assert(x == 1)",
            ),
            // Both admitted, but the second dooms the module.
            ("d", "mixed", FoundBy::Review, "assert(m == 1)"),
            ("d", "mixed", FoundBy::Test, "assert(m == 2)"),
            // The one module nothing has ever tested.
            ("d", "watched", FoundBy::Observation, "assert(w == 1)"),
        ]);
        assert_eq!(untested_modules(&c), vec![("watched".to_string(), 1)]);
    }

    #[test]
    fn untested_modules_count_only_review_and_observation_defects() {
        let c = admitted(&[
            ("d", "hot", FoundBy::Review, "assert(h == 1)"),
            ("d", "hot", FoundBy::Observation, "assert(h == 2)"),
            ("d", "hot", FoundBy::Observation, "assert(h == 3)"),
            // Found by a suite, so absent — and its count must not leak.
            ("d", "cold", FoundBy::CrossExamination, "assert(c == 1)"),
        ]);
        assert_eq!(untested_modules(&c), vec![("hot".to_string(), 3)]);
    }

    #[test]
    fn untested_modules_rank_by_count_descending_with_ties_broken_by_name() {
        let c = admitted(&[
            ("d", "beta", FoundBy::Observation, "assert(b == 1)"),
            ("d", "alpha", FoundBy::Review, "assert(a == 1)"),
            ("d", "gamma", FoundBy::Observation, "assert(g == 1)"),
            ("d", "gamma", FoundBy::Observation, "assert(g == 2)"),
        ]);
        assert_eq!(
            untested_modules(&c),
            vec![
                ("gamma".to_string(), 2),
                ("alpha".to_string(), 1),
                ("beta".to_string(), 1),
            ]
        );
    }

    #[test]
    fn defects_order_does_not_depend_on_admission_order() {
        let specs = [
            ("b", "quota", FoundBy::Test, "assert(q == 1)"),
            ("a", "slots", FoundBy::Review, "assert(s == 1)"),
            ("c", "proto", FoundBy::Observation, "assert(p == 1)"),
        ];
        let mut forward = Corpus::new();
        let mut backward = Corpus::new();
        for (i, (id, module, found_by, assertion)) in specs.iter().enumerate() {
            assert_eq!(
                forward.admit(defect(id, module, *found_by, assertion)),
                Ok(())
            );
            let (rid, rmodule, rfound_by, rassertion) = specs[specs.len() - 1 - i];
            assert_eq!(
                backward.admit(defect(rid, rmodule, rfound_by, rassertion)),
                Ok(())
            );
        }
        assert_eq!(
            ids(&forward),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert_eq!(forward.defects(), backward.defects());
    }

    #[test]
    fn by_module_returns_only_that_module_ordered_by_id() {
        let c = admitted(&[
            ("z", "quota", FoundBy::Test, "assert(q == 1)"),
            ("a", "quota", FoundBy::Review, "assert(q == 2)"),
            ("m", "slots", FoundBy::Observation, "assert(s == 1)"),
        ]);
        let quota: Vec<String> = c.by_module("quota").iter().map(|d| d.id.clone()).collect();
        assert_eq!(quota, vec!["a-1".to_string(), "z-0".to_string()]);
        assert!(c.by_module("nope").is_empty());
    }

    #[test]
    fn an_empty_corpus_is_empty_and_yields_no_views() {
        let c = Corpus::new();
        assert!(c.is_empty());
        assert_eq!(c.len(), 0);
        assert!(c.defects().is_empty());
        assert!(c.by_module("quota").is_empty());
        assert!(c.histogram().is_empty());
        assert!(untested_modules(&c).is_empty());
    }
}
