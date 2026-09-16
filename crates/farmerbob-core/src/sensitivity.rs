//! Defect sensitivity: measuring whether a candidate's TESTS are good.
//!
//! An implementation's test suite is part of the deliverable, and its quality
//! is measurable without any judge: inject known defects (mutants) into a
//! reference implementation, run the candidate's frozen suite against each
//! mutant, and count how many it catches. The fraction of the catchable
//! defects a suite catches is its *sensitivity*.
//!
//! Two subtleties govern the arithmetic, and both are about what must NOT be
//! counted:
//!
//! - **The denominator.** A mutant nothing detects either changed no
//!   observable behaviour (it is inert) or changed behaviour the specification
//!   is silent about. Neither is a miss on any candidate's part, so such
//!   mutants are excluded from every denominator rather than counted against
//!   every suite equally. A defect is admitted to the corpus only if the
//!   reference conformance suite catches it — that is what makes it a defect
//!   rather than a refactor — and the same filter applies one level down: a
//!   defect is *live* only if at least one admitted suite caught it
//!   ([`Corpus::live_defects`]); the rest are *inert*
//!   ([`Corpus::inert_defects`]) and are reported so a human can decide which
//!   they are.
//!
//! - **The baseline.** A suite that fails on the *unmutated* reference is
//!   broken. Its "detections" are meaningless — it would report every mutant
//!   as caught while detecting nothing — so it is disqualified
//!   ([`Corpus::disqualified`]), not credited with perfect sensitivity: it
//!   contributes to no numerator and no denominator, cannot make a defect
//!   live, and never appears in [`rank`].
//!
//! For the same reason an absent measurement is never folded into a measured
//! zero. [`Detection::DidNotRun`] shrinks only its own suite's denominator —
//! a suite that could not be executed against one mutant is unmeasured there,
//! not wrong — and [`sensitivity`] answers `None`, never `0.0` or `1.0`,
//! wherever no measurement exists: for a disqualified suite, for a suite that
//! measured no live defect, for a suite the corpus does not contain, and for
//! every suite when no defect is live, because a corpus that discriminates
//! nothing cannot rank anyone.
//!
//! Every function here is pure: execution happens elsewhere, and this module
//! only folds the results. No I/O, no clocks, no randomness.

use std::collections::BTreeSet;

/// A candidate defect, injected into the reference implementation.
///
/// Candidate because the corpus is what confirms it: a mutation no admitted
/// suite catches turned out to be inert, or to have changed behaviour the
/// specification is silent about, and drops out of every denominator (see
/// [`Corpus::inert_defects`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Defect {
    /// Stable name for this defect, unique within a [`Corpus`]. All scoring
    /// keys on it; the [`description`](Defect::description) is for humans.
    pub id: String,
    /// What the defect is, in prose. Reporting only: no measurement here
    /// reads it.
    pub description: String,
}

/// Whether one suite caught one defect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detection {
    /// The suite ran against this mutant and failed: it caught the defect.
    Caught,
    /// The suite ran against this mutant and passed: it missed the defect.
    Missed,
    /// The suite produced no measurement against this mutant — it did not
    /// build, crashed, or timed out. Neither a catch nor a miss: it shrinks
    /// only its own suite's denominator, because a suite that could not be
    /// executed here is unmeasured, not wrong.
    DidNotRun,
}

/// One suite's complete result set.
#[derive(Debug, Clone, PartialEq)]
pub struct SuiteRun {
    /// The suite's name, unique within a [`Corpus`].
    pub suite: String,
    /// Did this suite pass against the UNMUTATED reference? If not, it is
    /// disqualified: every detection it reports is meaningless, so it
    /// contributes to no numerator, no denominator, and no ranking.
    pub baseline_pass: bool,
    /// One entry per defect, by defect id.
    ///
    /// An entry a run omits is an absent measurement, folded exactly like
    /// [`Detection::DidNotRun`]. Should a malformed run repeat an id, the
    /// entries fold: the defect counts as caught for this suite if any entry
    /// says [`Detection::Caught`], and as measured if any entry is not
    /// [`Detection::DidNotRun`].
    pub detections: Vec<(String, Detection)>,
}

/// A defect no admitted suite caught is excluded from every denominator.
///
/// The unit of measurement: the injected [`Defect`]s plus every [`SuiteRun`]
/// executed against them. Built through [`Corpus::new`], which refuses data
/// that would make any count below ambiguous or silently wrong.
///
/// The corpus partitions its defects into the *live* ones at least one
/// admitted suite caught — demonstrably catchable, so missing one is a real
/// miss — and the *inert* ones no admitted suite caught, which are excluded
/// from every denominator and reported so a human can decide whether each is
/// inert or merely outside the specification.
#[derive(Debug, Clone, PartialEq)]
pub struct Corpus {
    defects: Vec<Defect>,
    runs: Vec<SuiteRun>,
}

impl Corpus {
    /// Build a corpus from the defects and the suite runs against them.
    ///
    /// Errors on a duplicate defect id, on a [`SuiteRun`] naming a defect not
    /// in `defects`, or on two runs sharing a suite name. Nothing is silently
    /// dropped: an ignored detection would quietly shrink a denominator, and
    /// a second run for one suite would leave every by-name query below
    /// ambiguous.
    ///
    /// A run need not report every defect — an omitted defect is an absent
    /// measurement, treated like [`Detection::DidNotRun`] — but every defect
    /// it does name must exist here.
    pub fn new(defects: Vec<Defect>, runs: Vec<SuiteRun>) -> Result<Self, String> {
        let mut seen_defects: BTreeSet<&str> = BTreeSet::new();
        for defect in &defects {
            if !seen_defects.insert(defect.id.as_str()) {
                return Err(format!("duplicate defect id: {}", defect.id));
            }
        }
        let mut seen_suites: BTreeSet<&str> = BTreeSet::new();
        for run in &runs {
            if !seen_suites.insert(run.suite.as_str()) {
                return Err(format!("duplicate suite name: {}", run.suite));
            }
            for (defect_id, _) in &run.detections {
                if !seen_defects.contains(defect_id.as_str()) {
                    return Err(format!(
                        "suite {} reports a defect not in the corpus: {defect_id}",
                        run.suite
                    ));
                }
            }
        }
        Ok(Self { defects, runs })
    }

    /// Defects at least one admitted suite caught. These form the denominator.
    ///
    /// In declaration order — the order the defects were given to
    /// [`Corpus::new`]. A defect "caught" only by disqualified suites is not
    /// here: a broken suite's detections prove nothing catchable.
    pub fn live_defects(&self) -> Vec<String> {
        self.defects
            .iter()
            .filter(|defect| self.is_live(&defect.id))
            .map(|defect| defect.id.clone())
            .collect()
    }

    /// Defects no admitted suite caught: inert, or the spec is silent.
    /// Excluded, and reported so a human can decide which.
    ///
    /// In declaration order. Together with [`Corpus::live_defects`] this
    /// partitions every defect in the corpus, exactly once.
    pub fn inert_defects(&self) -> Vec<String> {
        self.defects
            .iter()
            .filter(|defect| !self.is_live(&defect.id))
            .map(|defect| defect.id.clone())
            .collect()
    }

    /// Suites disqualified for failing the baseline, in declaration order.
    ///
    /// A disqualified suite contributes to no numerator and no denominator,
    /// cannot make a defect live, scores [`None`](sensitivity) rather than
    /// `0.0`, and never appears in [`rank`].
    pub fn disqualified(&self) -> Vec<String> {
        self.runs
            .iter()
            .filter(|run| !run.baseline_pass)
            .map(|run| run.suite.clone())
            .collect()
    }

    /// The runs that passed the baseline: the only ones whose detections
    /// count for anything.
    fn admitted_runs(&self) -> impl Iterator<Item = &SuiteRun> {
        self.runs.iter().filter(|run| run.baseline_pass)
    }

    /// The run recorded for `suite`, if the corpus contains one.
    fn run_for(&self, suite: &str) -> Option<&SuiteRun> {
        self.runs.iter().find(|run| run.suite == suite)
    }

    /// Whether any admitted suite caught this defect.
    fn is_live(&self, defect_id: &str) -> bool {
        self.admitted_runs().any(|run| caught(run, defect_id))
    }
}

/// Fraction of live defects this suite caught. `None` for a disqualified
/// suite, and `None` when there are no live defects — a corpus that
/// discriminates nothing cannot rank anyone, and 0.0 or 1.0 would both be
/// fabrications.
///
/// Also `None` when the corpus holds no run for `suite`, or when the run
/// measured no live defect at all (every one [`Detection::DidNotRun`] or
/// omitted): an absent measurement and a measured zero must be
/// distinguishable, and only [`Detection::Missed`] entries make a zero a
/// measurement. Where a score exists it lies in `0.0..=1.0`.
pub fn sensitivity(c: &Corpus, suite: &str) -> Option<f64> {
    let run = c.run_for(suite)?;
    if !run.baseline_pass {
        return None;
    }
    score(run, &c.live_defects())
}

/// Defects caught by exactly one suite. These are the most valuable rows in
/// the corpus: the places where one author tested something no one else
/// thought of.
///
/// Only admitted suites are counted, so a defect caught by one admitted suite
/// and any number of disqualified ones is still a unique catch, while a
/// defect whose only catchers are disqualified was never caught at all — it
/// is inert, and absent here. Returns `(defect_id, suite)` pairs sorted by
/// defect id.
pub fn unique_catches(c: &Corpus) -> Vec<(String, String)> {
    let mut unique = Vec::new();
    for defect in &c.defects {
        let catchers: Vec<&str> = c
            .admitted_runs()
            .filter(|run| caught(run, &defect.id))
            .map(|run| run.suite.as_str())
            .collect();
        if let [only] = catchers.as_slice() {
            unique.push((defect.id.clone(), (*only).to_string()));
        }
    }
    unique.sort_by(|a, b| a.0.cmp(&b.0));
    unique
}

/// Suites ranked by sensitivity, highest first, disqualified suites omitted.
/// Ties are broken by suite name so the ranking is deterministic, under any
/// input ordering of the defects, runs, or detections.
///
/// A suite with no sensitivity — with a non-empty live set, one that
/// measured no live defect — is omitted rather than given a fabricated
/// score. When no defect is live the ranking is empty: nobody can be ranked.
pub fn rank(c: &Corpus) -> Vec<(String, f64)> {
    let live = c.live_defects();
    let mut ranked: Vec<(String, f64)> = c
        .admitted_runs()
        .filter_map(|run| score(run, &live).map(|s| (run.suite.clone(), s)))
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
}

/// One run's catches over its measurements, counted across the live defects
/// only: inert defects leave every denominator, and [`Detection::DidNotRun`]
/// (like an omitted entry) leaves this run's.
///
/// `None` when there is nothing to divide by — an empty `live` set, or a run
/// that measured none of it. A catch is always a measurement, so where a
/// score exists it lies in `0.0..=1.0` and is never NaN.
fn score(run: &SuiteRun, live: &[String]) -> Option<f64> {
    let mut measured_count = 0usize;
    let mut caught_count = 0usize;
    for defect_id in live {
        if !measured(run, defect_id) {
            continue;
        }
        measured_count += 1;
        if caught(run, defect_id) {
            caught_count += 1;
        }
    }
    if measured_count == 0 {
        return None;
    }
    Some(caught_count as f64 / measured_count as f64)
}

/// Whether `run` caught `defect_id`: some entry for it says
/// [`Detection::Caught`].
fn caught(run: &SuiteRun, defect_id: &str) -> bool {
    run.detections
        .iter()
        .any(|(id, detection)| id == defect_id && *detection == Detection::Caught)
}

/// Whether `run` produced a real measurement of `defect_id`: some entry for
/// it is a catch or a miss. [`Detection::DidNotRun`] and absent entries are
/// both unmeasured.
fn measured(run: &SuiteRun, defect_id: &str) -> bool {
    run.detections
        .iter()
        .any(|(id, detection)| id == defect_id && *detection != Detection::DidNotRun)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defect(id: &str) -> Defect {
        Defect {
            id: id.to_string(),
            description: format!("injected defect {id}"),
        }
    }

    fn defects(ids: &[&str]) -> Vec<Defect> {
        ids.iter().map(|id| defect(id)).collect()
    }

    /// One run. `pattern` holds one character per entry of `ids`, in order:
    /// `C` caught, `M` missed, `N` did not run, `-` no entry at all.
    fn suite_run(suite: &str, ids: &[&str], pattern: &str, baseline_pass: bool) -> SuiteRun {
        assert_eq!(pattern.len(), ids.len(), "one pattern character per defect");
        let detections = ids
            .iter()
            .zip(pattern.chars())
            .filter_map(|(id, ch)| {
                let detection = match ch {
                    'C' => Detection::Caught,
                    'M' => Detection::Missed,
                    'N' => Detection::DidNotRun,
                    '-' => return None,
                    other => panic!("bad detection char: {other}"),
                };
                Some(((*id).to_string(), detection))
            })
            .collect();
        SuiteRun {
            suite: suite.to_string(),
            baseline_pass,
            detections,
        }
    }

    /// A corpus that must be well formed; tests use it for valid data only.
    fn corpus(ids: &[&str], runs: Vec<SuiteRun>) -> Corpus {
        Corpus::new(defects(ids), runs).expect("well-formed corpus")
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    // --- the disqualification rules ---------------------------------------

    #[test]
    fn baseline_failing_suite_is_disqualified_and_scores_none() {
        let ids = ["d1", "d2"];
        let c = corpus(
            &ids,
            vec![
                suite_run("broken", &ids, "CC", false),
                suite_run("honest", &ids, "CM", true),
            ],
        );

        assert_eq!(c.disqualified(), vec!["broken".to_string()]);
        assert_eq!(
            sensitivity(&c, "broken"),
            None,
            "a disqualified suite scores None, never a fabricated 1.0 or 0.0"
        );
        // The broken suite "caught" both defects, but only honest counts:
        // d1 is live, and honest's missed d2 is inert, so honest is 1 of 1.
        assert_eq!(c.live_defects(), vec!["d1".to_string()]);
        assert_eq!(sensitivity(&c, "honest"), Some(1.0));
    }

    #[test]
    fn defect_caught_only_by_a_disqualified_suite_is_inert() {
        let ids = ["d1", "d2"];
        let c = corpus(
            &ids,
            vec![
                suite_run("broken", &ids, "CC", false),
                suite_run("honest", &ids, "CM", true),
            ],
        );

        assert_eq!(
            c.inert_defects(),
            vec!["d2".to_string()],
            "a broken suite's catch cannot make a defect live"
        );
        assert_eq!(c.live_defects(), vec!["d1".to_string()]);
        assert!(
            unique_catches(&c)
                .iter()
                .all(|(_, suite)| suite != "broken"),
            "a disqualified suite is never a unique catcher"
        );
    }

    #[test]
    fn disqualified_suites_never_appear_in_rank() {
        let ids = ["d1", "d2"];
        let c = corpus(
            &ids,
            vec![
                suite_run("broken", &ids, "CC", false),
                suite_run("honest", &ids, "CM", true),
            ],
        );

        assert_eq!(rank(&c), vec![("honest".to_string(), 1.0)]);
        assert!(
            rank(&c).iter().all(|(suite, _)| suite != "broken"),
            "rank omits the disqualified, whatever they 'caught'"
        );
    }

    #[test]
    fn a_corpus_of_only_broken_suites_measures_nothing() {
        let ids = ["d1", "d2"];
        let c = corpus(&ids, vec![suite_run("broken", &ids, "CC", false)]);

        assert_eq!(c.disqualified(), vec!["broken".to_string()]);
        assert!(
            c.live_defects().is_empty(),
            "no admitted suite, nothing live"
        );
        assert_eq!(c.inert_defects(), vec!["d1".to_string(), "d2".to_string()]);
        assert_eq!(sensitivity(&c, "broken"), None);
        assert!(rank(&c).is_empty());
        assert!(unique_catches(&c).is_empty());
    }

    // --- the denominator rules ---------------------------------------------

    #[test]
    fn did_not_run_shrinks_only_its_own_denominator() {
        let ids = ["d1", "d2", "d3"];
        let c = corpus(
            &ids,
            vec![
                suite_run("full", &ids, "CCC", true),
                suite_run("partial", &ids, "CNM", true),
                suite_run("peer", &ids, "MCM", true),
            ],
        );

        // Every defect is live: full caught all three, and partial's DidNotRun
        // on d2 does not un-live what peer caught.
        assert_eq!(
            c.live_defects(),
            vec!["d1".to_string(), "d2".to_string(), "d3".to_string()]
        );
        // partial measured d1 and d3 only, and caught one of those two.
        let partial = sensitivity(&c, "partial").expect("partial measured live defects");
        assert!(
            close(partial, 0.5),
            "1 of 2 measured, not 1 of 3: {partial}"
        );
        // The DidNotRun shrank partial's denominator alone: peer still answers
        // over all three, and full still caught all three.
        let peer = sensitivity(&c, "peer").expect("peer measured everything");
        assert!(close(peer, 1.0 / 3.0), "peer is 1 of 3, unshrunk: {peer}");
        assert_eq!(sensitivity(&c, "full"), Some(1.0));
    }

    #[test]
    fn a_suite_that_measured_no_live_defect_scores_none() {
        let ids = ["d1", "d2"];
        let c = corpus(
            &ids,
            vec![
                suite_run("sharp", &ids, "CC", true),
                suite_run("crashed", &ids, "NN", true),
            ],
        );

        // crashed passed the baseline but produced no measurement of either
        // live defect: unmeasured, which is not a measured zero.
        assert_eq!(sensitivity(&c, "crashed"), None);
        assert_eq!(
            rank(&c),
            vec![("sharp".to_string(), 1.0)],
            "an unmeasured suite is omitted from the ranking, not ranked at 0.0"
        );
    }

    #[test]
    fn empty_live_set_gives_none_not_zero() {
        let ids = ["d1"];
        let c = corpus(&ids, vec![suite_run("blind", &ids, "M", true)]);

        assert!(c.live_defects().is_empty());
        assert_eq!(c.inert_defects(), vec!["d1".to_string()]);
        assert_eq!(
            sensitivity(&c, "blind"),
            None,
            "no live defects: a corpus that discriminates nothing ranks no one"
        );
        assert!(rank(&c).is_empty());
    }

    #[test]
    fn zero_catch_admitted_suite_gives_some_zero() {
        let ids = ["d1", "d2"];
        let c = corpus(
            &ids,
            vec![
                suite_run("sharp", &ids, "CC", true),
                suite_run("blind", &ids, "MM", true),
            ],
        );

        // blind ran against both live defects and caught neither: a genuine,
        // measured zero — distinguishable from every None case above.
        assert_eq!(sensitivity(&c, "blind"), Some(0.0));
        assert_eq!(sensitivity(&c, "sharp"), Some(1.0));
        assert_eq!(
            rank(&c),
            vec![("sharp".to_string(), 1.0), ("blind".to_string(), 0.0)]
        );
    }

    #[test]
    fn inert_defects_leave_every_denominator() {
        let ids = ["d1", "d2"];
        let c = corpus(
            &ids,
            vec![
                suite_run("a", &ids, "CM", true),
                suite_run("b", &ids, "MM", true),
            ],
        );

        // d2 is inert: nobody caught it, so it is not a miss on anyone.
        assert_eq!(c.inert_defects(), vec!["d2".to_string()]);
        assert_eq!(
            sensitivity(&c, "a"),
            Some(1.0),
            "a is 1 of 1, not 1 of 2: the inert defect left its denominator"
        );
        assert_eq!(sensitivity(&c, "b"), Some(0.0), "b is 0 of 1, not 0 of 2");
    }

    #[test]
    fn a_missing_entry_is_unmeasured_not_a_miss() {
        let ids = ["d1", "d2"];
        let c = corpus(
            &ids,
            vec![
                suite_run("sharp", &ids, "CC", true),
                suite_run("sparse", &ids, "C-", true),
            ],
        );

        // sparse says nothing about d2 at all. Silence is an absent
        // measurement, folded like DidNotRun: sparse is 1 of 1, not 1 of 2.
        assert_eq!(sensitivity(&c, "sparse"), Some(1.0));
        assert_eq!(c.live_defects(), vec!["d1".to_string(), "d2".to_string()]);
    }

    #[test]
    fn a_defect_any_admitted_suite_caught_is_live() {
        let ids = ["d1"];
        let c = corpus(
            &ids,
            vec![
                suite_run("blind", &ids, "M", true),
                suite_run("sharp", &ids, "C", true),
            ],
        );

        assert_eq!(c.live_defects(), vec!["d1".to_string()]);
        assert!(c.inert_defects().is_empty());
        // One admitted catcher is enough to make missing it a real miss.
        assert_eq!(sensitivity(&c, "blind"), Some(0.0));
        assert_eq!(sensitivity(&c, "sharp"), Some(1.0));
    }

    // --- unique catches -----------------------------------------------------

    #[test]
    fn unique_catches_finds_the_sole_catcher() {
        let ids = ["d1", "d2", "d3"];
        let c = corpus(
            &ids,
            vec![
                suite_run("a", &ids, "CMM", true),
                suite_run("b", &ids, "CCM", true),
            ],
        );

        // d1: both caught it. d2: only b. d3: nobody — inert, not a catch.
        assert_eq!(
            unique_catches(&c),
            vec![("d2".to_string(), "b".to_string())],
            "exactly the defect one author alone tested for"
        );
    }

    #[test]
    fn unique_catches_counts_only_admitted_suites() {
        let ids = ["d1", "d2"];
        let c = corpus(
            &ids,
            vec![
                suite_run("broken", &ids, "CC", false),
                suite_run("solo", &ids, "CM", true),
            ],
        );

        // d1: caught by solo and by disqualified broken — the disqualified
        // catch does not count, so solo is still the sole catcher.
        // d2: caught only by broken — never caught at all, so no pair.
        assert_eq!(
            unique_catches(&c),
            vec![("d1".to_string(), "solo".to_string())]
        );
        assert_eq!(c.inert_defects(), vec!["d2".to_string()]);
    }

    #[test]
    fn unique_catches_is_sorted_by_defect_id() {
        // Declared out of lexicographic order on purpose.
        let ids = ["zebra", "apple", "mango"];
        let c = corpus(&ids, vec![suite_run("solo", &ids, "CCC", true)]);

        assert_eq!(
            unique_catches(&c),
            vec![
                ("apple".to_string(), "solo".to_string()),
                ("mango".to_string(), "solo".to_string()),
                ("zebra".to_string(), "solo".to_string()),
            ]
        );
    }

    // --- ranking -------------------------------------------------------------

    #[test]
    fn rank_orders_by_sensitivity_highest_first() {
        let ids = ["d1", "d2", "d3", "d4"];
        let c = corpus(
            &ids,
            vec![
                suite_run("low", &ids, "MMMM", true),
                suite_run("high", &ids, "CCCC", true),
                suite_run("mid", &ids, "CCMM", true),
            ],
        );

        let ranked = rank(&c);
        let names: Vec<&str> = ranked.iter().map(|(suite, _)| suite.as_str()).collect();
        assert_eq!(names, vec!["high", "mid", "low"]);
        assert_eq!(ranked[0].1, 1.0);
        assert!(close(ranked[1].1, 0.5), "mid is 2 of 4: {}", ranked[1].1);
        assert_eq!(ranked[2].1, 0.0);
        for window in ranked.windows(2) {
            assert!(
                window[0].1 >= window[1].1,
                "scores never increase: {ranked:?}"
            );
        }
    }

    #[test]
    fn rank_is_deterministic_when_two_suites_tie() {
        let ids = ["d1", "d2"];
        let c = corpus(
            &ids,
            vec![
                suite_run("zeta", &ids, "CM", true),
                suite_run("alpha", &ids, "MC", true),
            ],
        );

        // Both caught one live defect of two: a tie, broken by name.
        assert_eq!(
            rank(&c),
            vec![("alpha".to_string(), 0.5), ("zeta".to_string(), 0.5)]
        );
        for _ in 0..8 {
            assert_eq!(rank(&c)[0].0, "alpha", "same corpus, same ranking");
        }
    }

    #[test]
    fn rank_is_deterministic_under_any_input_ordering() {
        let a = corpus(
            &["d1", "d2", "d3"],
            vec![
                suite_run("high", &["d1", "d2", "d3"], "CCC", true),
                suite_run("mid", &["d1", "d2", "d3"], "CNM", true),
                suite_run("low", &["d1", "d2", "d3"], "MMM", true),
                suite_run("broken", &["d1", "d2", "d3"], "CCC", false),
            ],
        );
        // The same data with defects, runs, and detection entries permuted.
        let b = corpus(
            &["d3", "d1", "d2"],
            vec![
                suite_run("low", &["d3", "d1", "d2"], "MMM", true),
                suite_run("broken", &["d3", "d1", "d2"], "CCC", false),
                suite_run("high", &["d3", "d1", "d2"], "CCC", true),
                suite_run("mid", &["d3", "d1", "d2"], "MCN", true),
            ],
        );

        let expected = vec![
            ("high".to_string(), 1.0),
            ("mid".to_string(), 0.5),
            ("low".to_string(), 0.0),
        ];
        assert_eq!(rank(&a), expected);
        assert_eq!(rank(&b), expected, "input ordering must not leak into rank");
        assert_eq!(unique_catches(&a), unique_catches(&b));
    }

    // --- construction errors --------------------------------------------------

    #[test]
    fn unknown_defect_id_is_rejected() {
        let result = Corpus::new(
            defects(&["d1"]),
            vec![suite_run("a", &["d1", "ghost"], "CM", true)],
        );
        assert!(
            result.is_err(),
            "a detection naming an absent defect must be rejected, not ignored"
        );
    }

    #[test]
    fn duplicate_defect_id_is_rejected() {
        let result = Corpus::new(vec![defect("d1"), defect("d1")], vec![]);
        assert!(
            result.is_err(),
            "scoring keys on defect ids, so they must be unique"
        );
    }

    #[test]
    fn duplicate_suite_name_is_rejected() {
        let ids = ["d1"];
        let result = Corpus::new(
            defects(&ids),
            vec![
                suite_run("a", &ids, "C", true),
                suite_run("a", &ids, "M", true),
            ],
        );
        assert!(
            result.is_err(),
            "two runs for one suite would leave every by-name query ambiguous"
        );
    }

    #[test]
    fn a_run_may_omit_defects_and_an_empty_corpus_is_well_formed() {
        let ids = ["d1", "d2"];
        let c = corpus(&ids, vec![suite_run("sparse", &ids, "C-", true)]);
        assert_eq!(c.live_defects(), vec!["d1".to_string()]);

        let empty = corpus(&[], vec![]);
        assert!(empty.live_defects().is_empty());
        assert!(empty.inert_defects().is_empty());
        assert!(empty.disqualified().is_empty());
        assert!(rank(&empty).is_empty());
        assert!(unique_catches(&empty).is_empty());
        assert_eq!(sensitivity(&empty, "anyone"), None);
    }

    // --- cross-cutting invariants ----------------------------------------------

    #[test]
    fn sensitivity_of_a_suite_not_in_the_corpus_is_none() {
        let ids = ["d1"];
        let c = corpus(&ids, vec![suite_run("a", &ids, "C", true)]);
        assert_eq!(sensitivity(&c, "nobody"), None);
    }

    #[test]
    fn live_and_inert_partition_the_defects() {
        let ids = ["d1", "d2", "d3", "d4"];
        let c = corpus(
            &ids,
            vec![
                suite_run("a", &ids, "CCMN", true),
                suite_run("b", &ids, "MMNM", true),
                suite_run("broken", &ids, "CCCC", false),
            ],
        );

        let mut all: Vec<String> = c.live_defects();
        all.extend(c.inert_defects());
        all.sort();
        let mut expected: Vec<String> = ids.iter().map(|id| (*id).to_string()).collect();
        expected.sort();
        assert_eq!(all, expected, "every defect exactly once, live or inert");
        assert_eq!(c.live_defects(), vec!["d1".to_string(), "d2".to_string()]);
        assert_eq!(c.inert_defects(), vec!["d3".to_string(), "d4".to_string()]);
    }

    #[test]
    fn no_score_that_exists_is_nan_or_out_of_range() {
        let ids = ["d1", "d2", "d3"];
        let c = corpus(
            &ids,
            vec![
                suite_run("a", &ids, "CCN", true),
                suite_run("b", &ids, "NMM", true),
                suite_run("broken", &ids, "CCC", false),
            ],
        );

        for suite in ["a", "b", "broken", "ghost"] {
            if let Some(s) = sensitivity(&c, suite) {
                assert!(
                    s.is_finite() && (0.0..=1.0).contains(&s),
                    "{suite} scored {s}: must be a finite fraction"
                );
            }
        }
        for (suite, s) in rank(&c) {
            assert!(
                s.is_finite() && (0.0..=1.0).contains(&s),
                "{suite} ranked at {s}: must be a finite fraction"
            );
        }
    }
}
