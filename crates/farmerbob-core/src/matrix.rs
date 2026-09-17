//! Reading an N×N cross-examination matrix.
//!
//! N candidates attack one task, so a run produces N implementations and N
//! test suites. Running every suite against every implementation yields a
//! square of [`Cell`]s — row = implementation, column = suite — and this
//! module is the pure reader for that square. It names the four shapes the
//! field can take ([`shape`]), grades each suite by what it does to the
//! others ([`suite_quality`]), scores each implementation by what it survives
//! ([`survival`]), and lists what each suite caught ([`discoveries`]).
//!
//! The diagonal is the invariant that makes the rest trustworthy: each arm's
//! suite passed on its own code in its own worktree before the matrix was
//! assembled. A diagonal that is not all-[`Cell::Pass`] is a transplant
//! error, so [`shape`] reports [`Shape::Void`] and refuses every other
//! reading.
//!
//! [`Cell::NoCompile`] is a missing measurement, never a failure. It is
//! excluded from every numerator and every denominator, so a suite that
//! could not be built somewhere neither helps nor hurts the arms involved.
//!
//! Pure logic: the caller runs the cells; this module never spawns a process.

use std::collections::HashSet;

/// Outcome of running one suite against one implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    /// The suite ran and every assertion held.
    Pass,
    /// The suite ran and at least one assertion broke.
    Fail,
    /// The suite could not be built against this implementation: a missing
    /// measurement, not a failure, and excluded from every count.
    NoCompile,
}

/// Square matrix over one candidate set. Row = implementation, column = suite.
///
/// The arms are the candidate names; the same name indexes a row (an
/// implementation) and a column (that arm's suite). The matrix does not have
/// to be symmetric: arm A's implementation may pass arm B's suite while B's
/// implementation fails A's.
#[derive(Debug, Clone, PartialEq)]
pub struct Matrix {
    arms: Vec<String>,
    cells: Vec<Cell>,
}

impl Matrix {
    /// Builds the matrix. `cells` is row-major: implementation 0 against
    /// every suite, then implementation 1, and so on.
    ///
    /// Errors when `cells` does not hold exactly `arms.len() * arms.len()`
    /// outcomes, or when two arms share a name.
    pub fn new(arms: Vec<String>, cells: Vec<Cell>) -> Result<Self, String> {
        let n = arms.len();
        if cells.len() != n * n {
            return Err(format!(
                "cells must be arms x arms: {n} arms need {} cells, got {}",
                n * n,
                cells.len()
            ));
        }
        let mut seen = HashSet::with_capacity(n);
        if arms.iter().any(|arm| !seen.insert(arm.as_str())) {
            return Err(format!("duplicate arm name among {arms:?}"));
        }
        Ok(Self { arms, cells })
    }

    /// The outcome of `implementation` (row) under `suite` (column).
    pub fn get(&self, implementation: &str, suite: &str) -> Option<Cell> {
        let row = self.index(implementation)?;
        let col = self.index(suite)?;
        self.cells.get(row * self.arms.len() + col).copied()
    }

    /// The candidate arms, in matrix order.
    pub fn arms(&self) -> &[String] {
        &self.arms
    }

    fn index(&self, arm: &str) -> Option<usize> {
        self.arms.iter().position(|a| a == arm)
    }
}

/// What the matrix as a whole says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    /// The diagonal is not all-Pass. Nothing else may be read.
    Void {
        /// Every arm whose own suite did not pass on its own code, sorted.
        broken: Vec<String>,
    },
    /// Every off-diagonal cell passed. The arms agree; whether they are
    /// correct is UNKNOWN and no cell in this matrix bears on it.
    Consensus {
        /// Arms in the matrix, sorted ascending. Never empty when this
        /// variant is returned for a non-empty matrix; the 0-arm matrix
        /// reports it with an empty list, as it always has.
        arms: Vec<String>,
    },
    /// Camps that pass within themselves and fail across. Names each camp,
    /// sorted.
    SpecAmbiguous {
        /// The camps; names within a camp sorted, camps sorted among
        /// themselves. Never reported for fewer than four arms.
        camps: Vec<Vec<String>>,
    },
    /// Ordinary: some suites discriminate, some are over-fitted.
    Discriminating,
}

/// Classify. `Void` is checked first and suppresses every other reading.
///
/// `SpecAmbiguous` requires at least four arms and at least two camps, where
/// every member of a camp passes every suite in its own camp and fails every
/// suite outside it. With two or three arms a partition is indistinguishable
/// from ordinary disagreement and is not reported. A `NoCompile` anywhere off
/// the diagonal keeps the field un-classifiable: "fails every suite outside
/// it" cannot be affirmed over a missing measurement, so the shape falls back
/// to [`Shape::Discriminating`].
pub fn shape(m: &Matrix) -> Shape {
    let n = m.arms.len();

    let mut broken: Vec<String> = (0..n)
        .filter(|&i| m.cells[i * n + i] != Cell::Pass)
        .map(|i| m.arms[i].clone())
        .collect();
    if !broken.is_empty() {
        broken.sort();
        return Shape::Void { broken };
    }

    if m.cells.iter().all(|&c| c == Cell::Pass) {
        let mut arms = m.arms.clone();
        arms.sort();
        return Shape::Consensus { arms };
    }

    let fully_measured = (0..n * n).all(|k| {
        let (row, col) = (k / n, k % n);
        row == col || m.cells[k] != Cell::NoCompile
    });
    if n >= 4 && fully_measured && let Some(camps) = camps(m) {
        return Shape::SpecAmbiguous { camps };
    }

    Shape::Discriminating
}

/// Camps of the mutual-pass relation, when that relation partitions the field
/// cleanly: same camp iff both directions pass, different camps iff both
/// directions fail. `None` when the arms do not decompose that way — a pair
/// that neither cleanly agrees nor cleanly disagrees.
///
/// Seeding a camp from the first unassigned arm and taking every arm that
/// mutually passes it reproduces the exact partition when one exists; the
/// pairwise verification below rejects every other arrangement.
fn camps(m: &Matrix) -> Option<Vec<Vec<String>>> {
    let n = m.arms.len();
    let together =
        |i: usize, j: usize| m.cells[i * n + j] == Cell::Pass && m.cells[j * n + i] == Cell::Pass;

    let mut camp_of: Vec<Option<usize>> = vec![None; n];
    let mut member_lists: Vec<Vec<usize>> = Vec::new();
    for seed in 0..n {
        if camp_of[seed].is_some() {
            continue;
        }
        let id = member_lists.len();
        camp_of[seed] = Some(id);
        let mut members = vec![seed];
        for (other, assigned) in camp_of.iter_mut().enumerate().skip(seed + 1) {
            if assigned.is_none() && together(seed, other) {
                *assigned = Some(id);
                members.push(other);
            }
        }
        member_lists.push(members);
    }

    if member_lists.len() < 2 {
        return None;
    }
    for i in 0..n {
        for j in (i + 1)..n {
            let same = camp_of[i] == camp_of[j];
            if same && !together(i, j) {
                return None;
            }
            if !same && (m.cells[i * n + j] != Cell::Fail || m.cells[j * n + i] != Cell::Fail) {
                return None;
            }
        }
    }

    let mut named: Vec<Vec<String>> = member_lists
        .into_iter()
        .map(|members| {
            let mut names: Vec<String> = members.into_iter().map(|i| m.arms[i].clone()).collect();
            names.sort();
            names
        })
        .collect();
    named.sort();
    Some(named)
}

/// Whether a shape's classification rests on evidence about the IMPLEMENTATIONS.
///
/// The phrasing matters and an earlier version had it wrong. It read "evidence that could
/// have contradicted it", and a critic showed that definition contradicts the rule it
/// justifies: a `Void` matrix's broken diagonal cells COULD have passed, so by that wording
/// Void would be discriminating, while the rule says it is not. The rule is right; the
/// rationale was wrong. `Void` rests on evidence about the INSTRUMENT, and `Consensus` on
/// agreement a shared fault produces just as readily as correctness. Neither says anything
/// about the implementations, which is the question this answers.
///   (credit or-ling-30-flash, on `consensus`)
///
/// `false` for [`Shape::Consensus`] and [`Shape::Void`], `true` for every
/// other variant. A `Consensus` matrix only shows the arms agree; with every
/// suite passing on every implementation there is no cell that could have
/// caught a fault shared by all of them, so its agreement says nothing about
/// correctness. A `Void` matrix has a broken diagonal — an instrument that
/// cannot run against the code it shipped with — so nothing it reports is
/// trustworthy either.
///
/// The match is exhaustive with no wildcard arm on purpose: adding a variant
/// to [`Shape`] becomes a compile error here rather than a silent `true`.
pub fn is_discriminating(shape: &Shape) -> bool {
    match shape {
        Shape::Void { .. } => false,
        Shape::Consensus { .. } => false,
        Shape::SpecAmbiguous { .. } => true,
        Shape::Discriminating => true,
    }
}

/// How one arm's SUITE behaved, judged across the whole matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuiteQuality {
    /// Splits the field: fails on at least one measured rival, but not on a
    /// strict majority of them. Its failures carry information.
    Discriminating,
    /// Encodes its author's implementation rather than the spec: fails on a
    /// strict majority of the other implementations. Its failures carry no
    /// information about the arms it breaks.
    OverFitted,
    /// Fails on nobody. Nothing discriminates, so the suite says nothing
    /// about anyone.
    Uninformative,
}

/// Classify one arm's suite. Self-pairs are excluded from every count: an
/// author's suite passing on its own code says nothing about anyone.
///
/// OverFitted when it fails on a strict majority of the others; Uninformative
/// when it fails on none; Discriminating otherwise. `NoCompile` counts in
/// neither side of the majority: a suite that could not be built on an arm
/// has not failed on it. Returns `None` for an arm not in the matrix.
pub fn suite_quality(m: &Matrix, arm: &str) -> Option<SuiteQuality> {
    let col = m.index(arm)?;
    let n = m.arms.len();
    let mut measured = 0usize;
    let mut fails = 0usize;
    for row in 0..n {
        if row == col {
            continue;
        }
        match m.cells[row * n + col] {
            Cell::Pass => measured += 1,
            Cell::Fail => {
                measured += 1;
                fails += 1;
            }
            Cell::NoCompile => {}
        }
    }
    let quality = if fails * 2 > measured {
        SuiteQuality::OverFitted
    } else if fails == 0 {
        SuiteQuality::Uninformative
    } else {
        SuiteQuality::Discriminating
    };
    Some(quality)
}

/// Fraction of OTHER arms' discriminating suites this implementation passes.
/// `None` when no other arm has a discriminating suite — there is nothing to
/// survive — and also when every discriminating rival suite was unmeasured
/// (`NoCompile`) against this implementation, leaving nothing measured to
/// survive.
pub fn survival(m: &Matrix, arm: &str) -> Option<f64> {
    let row = m.index(arm)?;
    let n = m.arms.len();
    let mut measured = 0usize;
    let mut passed = 0usize;
    for col in 0..n {
        if col == row {
            continue;
        }
        let rival = &m.arms[col];
        if suite_quality(m, rival) != Some(SuiteQuality::Discriminating) {
            continue;
        }
        match m.cells[row * n + col] {
            Cell::Pass => {
                measured += 1;
                passed += 1;
            }
            Cell::Fail => measured += 1,
            Cell::NoCompile => {}
        }
    }
    if measured == 0 {
        None
    } else {
        Some(passed as f64 / measured as f64)
    }
}

/// Implementations this arm's suite breaks, excluding itself, sorted.
/// A `NoCompile` cell is not a break. An arm not in the matrix has broken
/// nothing.
pub fn discoveries(m: &Matrix, arm: &str) -> Vec<String> {
    let col = match m.index(arm) {
        Some(col) => col,
        None => return Vec::new(),
    };
    let n = m.arms.len();
    let mut found: Vec<String> = (0..n)
        .filter(|&row| row != col && m.cells[row * n + col] == Cell::Fail)
        .map(|row| m.arms[row].clone())
        .collect();
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::{
        Cell, Matrix, Shape, SuiteQuality, discoveries, is_discriminating, shape, suite_quality,
        survival,
    };

    const P: Cell = Cell::Pass;
    const F: Cell = Cell::Fail;
    const X: Cell = Cell::NoCompile;

    fn m(arms: &[&str], rows: &[&[Cell]]) -> Matrix {
        Matrix::new(
            arms.iter().map(|a| (*a).to_string()).collect(),
            rows.iter().flat_map(|r| r.iter().copied()).collect(),
        )
        .unwrap()
    }

    /// Order-insensitive comparison for camp lists: sorts names within each
    /// camp and the camps themselves.
    fn as_camp_sets(camps: Vec<Vec<String>>) -> Vec<Vec<String>> {
        let mut sets: Vec<Vec<String>> = camps
            .into_iter()
            .map(|mut camp| {
                camp.sort();
                camp
            })
            .collect();
        sets.sort();
        sets
    }

    fn sorted(names: Vec<String>) -> Vec<String> {
        let mut s = names;
        s.sort();
        s
    }

    fn campf(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_string()).collect()
    }

    /// Four arms, two clean camps {a,b} and {c,d}, given in an order that
    /// does not match the camp order.
    fn partition() -> Matrix {
        m(
            &["c", "a", "d", "b"],
            &[&[P, F, P, F], &[F, P, F, P], &[P, F, P, F], &[F, P, F, P]],
        )
    }

    #[test]
    fn a_broken_diagonal_voids_everything() {
        let matrix = m(&["a", "b", "c"], &[&[P, P, P], &[F, F, P], &[P, P, P]]);
        assert_eq!(
            shape(&matrix),
            Shape::Void {
                broken: vec!["b".to_string()]
            }
        );
    }

    #[test]
    fn void_names_every_broken_arm() {
        let matrix = m(&["c", "a", "b"], &[&[X, P, P], &[P, P, P], &[P, F, F]]);
        match shape(&matrix) {
            Shape::Void { broken } => {
                assert_eq!(sorted(broken), vec!["b".to_string(), "c".to_string()]);
            }
            other => panic!("expected Void, got {other:?}"),
        }
    }

    #[test]
    fn void_suppresses_every_other_reading() {
        // A perfect two-camp partition on four arms — except one arm's own
        // suite failed on its own code, so nothing else may be read.
        let matrix = m(
            &["a", "b", "c", "d"],
            &[&[P, P, F, F], &[P, F, F, F], &[F, F, P, P], &[F, F, P, P]],
        );
        match shape(&matrix) {
            Shape::Void { broken } => {
                assert_eq!(sorted(broken), vec!["b".to_string()]);
            }
            other => panic!("expected Void to suppress the partition, got {other:?}"),
        }
    }

    #[test]
    fn two_arm_partition_is_not_spec_ambiguous() {
        let matrix = m(&["a", "b"], &[&[P, F], &[F, P]]);
        assert_eq!(shape(&matrix), Shape::Discriminating);
    }

    #[test]
    fn three_arm_partition_is_not_spec_ambiguous() {
        let matrix = m(&["a", "b", "c"], &[&[P, F, F], &[F, P, F], &[F, F, P]]);
        assert_eq!(shape(&matrix), Shape::Discriminating);
    }

    #[test]
    fn four_arm_partition_is_spec_ambiguous() {
        let matrix = partition();
        assert_eq!(
            shape(&matrix),
            Shape::SpecAmbiguous {
                camps: vec![campf(&["a", "b"]), campf(&["c", "d"])]
            }
        );
    }

    #[test]
    fn camps_match_regardless_of_arm_order() {
        let matrix = m(
            &["d", "c", "b", "a"],
            &[&[P, P, F, F], &[P, P, F, F], &[F, F, P, P], &[F, F, P, P]],
        );
        assert_eq!(
            as_camp_sets(match shape(&matrix) {
                Shape::SpecAmbiguous { camps } => camps,
                other => panic!("expected SpecAmbiguous, got {other:?}"),
            }),
            vec![campf(&["a", "b"]), campf(&["c", "d"])]
        );
    }

    #[test]
    fn non_transitive_agreement_is_not_a_partition() {
        // a and b agree, b and c agree, but a and c fail across: no camps.
        let matrix = m(
            &["a", "b", "c", "d"],
            &[&[P, P, F, F], &[P, P, P, F], &[F, P, P, F], &[F, F, F, P]],
        );
        assert_eq!(shape(&matrix), Shape::Discriminating);
    }

    #[test]
    fn asymmetric_cross_pair_is_not_a_partition() {
        // Two clean camps except a passes c's suite while c fails a's: the
        // boundary does not hold in both directions.
        let matrix = m(
            &["a", "b", "c", "d"],
            &[&[P, P, P, F], &[P, P, F, F], &[F, F, P, P], &[F, F, P, P]],
        );
        assert_eq!(shape(&matrix), Shape::Discriminating);
    }

    #[test]
    fn consensus_when_every_cell_passes() {
        let matrix = m(&["a", "b", "c"], &[&[P, P, P], &[P, P, P], &[P, P, P]]);
        assert_eq!(
            shape(&matrix),
            Shape::Consensus {
                arms: vec!["a".to_string(), "b".to_string(), "c".to_string()]
            }
        );
    }

    #[test]
    fn one_nocompile_prevents_consensus() {
        let matrix = m(&["a", "b", "c"], &[&[P, P, P], &[P, P, X], &[P, P, P]]);
        assert_eq!(shape(&matrix), Shape::Discriminating);
    }

    #[test]
    fn single_arm_consensus() {
        let matrix = m(&["solo"], &[&[P]]);
        assert_eq!(
            shape(&matrix),
            Shape::Consensus {
                arms: vec!["solo".to_string()]
            }
        );
        assert_eq!(survival(&matrix, "solo"), None);
    }

    #[test]
    fn suite_failing_one_of_four_is_discriminating() {
        // a's suite: passes b, d, e; fails c only.
        let matrix = m(
            &["a", "b", "c", "d", "e"],
            &[
                &[P, P, P, P, P],
                &[P, P, P, P, P],
                &[F, P, P, P, P],
                &[P, P, P, P, P],
                &[P, P, P, P, P],
            ],
        );
        assert_eq!(
            suite_quality(&matrix, "a"),
            Some(SuiteQuality::Discriminating)
        );
    }

    #[test]
    fn suite_failing_three_of_four_is_overfitted() {
        // a's suite: fails b, c, d; passes e only.
        let matrix = m(
            &["a", "b", "c", "d", "e"],
            &[
                &[P, P, P, P, P],
                &[F, P, P, P, P],
                &[F, P, P, P, P],
                &[F, P, P, P, P],
                &[P, P, P, P, P],
            ],
        );
        assert_eq!(suite_quality(&matrix, "a"), Some(SuiteQuality::OverFitted));
    }

    #[test]
    fn exact_half_is_not_a_majority() {
        // Fails on 2 of 4 measured others: a tie is not a strict majority.
        let matrix = m(
            &["a", "b", "c", "d", "e"],
            &[
                &[P, P, P, P, P],
                &[F, P, P, P, P],
                &[F, P, P, P, P],
                &[P, P, P, P, P],
                &[P, P, P, P, P],
            ],
        );
        assert_eq!(
            suite_quality(&matrix, "a"),
            Some(SuiteQuality::Discriminating)
        );
    }

    #[test]
    fn nocompile_is_not_a_failure_for_quality() {
        // Fails on 2, unmeasured on 2, passes on 1: the majority is taken
        // over the three measured arms, so 2 of 3 is over-fitted.
        let matrix = m(
            &["s", "a", "b", "c", "d", "e"],
            &[
                &[P, P, P, P, P, P],
                &[F, P, P, P, P, P],
                &[F, P, P, P, P, P],
                &[X, P, P, P, P, P],
                &[X, P, P, P, P, P],
                &[P, P, P, P, P, P],
            ],
        );
        assert_eq!(suite_quality(&matrix, "s"), Some(SuiteQuality::OverFitted));
    }

    #[test]
    fn suite_failing_on_none_is_uninformative() {
        // Passes on b and c, unmeasured on d and e: fails on none.
        let matrix = m(
            &["s", "b", "c", "d", "e"],
            &[
                &[P, P, P, P, P],
                &[P, P, P, P, P],
                &[P, P, P, P, P],
                &[X, P, P, P, P],
                &[X, P, P, P, P],
            ],
        );
        assert_eq!(
            suite_quality(&matrix, "s"),
            Some(SuiteQuality::Uninformative)
        );
    }

    #[test]
    fn unmeasured_everywhere_is_uninformative_not_overfitted() {
        // Every rival failed to build this suite: nothing was measured.
        let matrix = m(&["s", "b", "c"], &[&[P, P, P], &[X, P, P], &[X, P, P]]);
        assert_eq!(
            suite_quality(&matrix, "s"),
            Some(SuiteQuality::Uninformative)
        );
    }

    #[test]
    fn quality_is_none_for_unknown_arm() {
        let matrix = m(&["a"], &[&[P]]);
        assert_eq!(suite_quality(&matrix, "nope"), None);
    }

    #[test]
    fn survival_is_fraction_of_discriminating_rivals_passed() {
        // Every rival suite discriminates; `me` passes x and z, fails y.
        let matrix = m(
            &["me", "x", "y", "z"],
            &[&[P, P, F, P], &[P, P, P, F], &[P, F, P, P], &[P, P, P, P]],
        );
        let s = survival(&matrix, "me").unwrap();
        assert!((s - 2.0 / 3.0).abs() < 1e-9, "survival was {s}");
    }

    #[test]
    fn survival_is_none_when_no_rival_suite_discriminates() {
        // `over`'s suite fails on both other rows (over-fitted); `unin`'s
        // fails on none. `me` fails both suites and still has nothing to
        // survive.
        let matrix = m(
            &["me", "over", "unin"],
            &[&[P, F, P], &[P, P, P], &[P, F, P]],
        );
        assert_eq!(survival(&matrix, "me"), None);
    }

    #[test]
    fn survival_counts_only_discriminating_rivals() {
        // `over` is over-fitted, `unin` uninformative; only `d`'s suite
        // discriminates, and `me` fails it.
        let matrix = m(
            &["me", "over", "unin", "d"],
            &[&[P, F, P, F], &[F, P, P, P], &[P, P, P, P], &[F, F, P, P]],
        );
        assert_eq!(
            suite_quality(&matrix, "over"),
            Some(SuiteQuality::OverFitted)
        );
        assert_eq!(
            suite_quality(&matrix, "unin"),
            Some(SuiteQuality::Uninformative)
        );
        assert_eq!(
            suite_quality(&matrix, "d"),
            Some(SuiteQuality::Discriminating)
        );
        assert_eq!(survival(&matrix, "me"), Some(0.0));
    }

    #[test]
    fn survival_excludes_nocompile_from_both_sides() {
        // Three discriminating rivals; `me` is unmeasured against x, passes
        // y, fails z: one of two measured.
        let matrix = m(
            &["me", "x", "y", "z"],
            &[&[P, X, P, F], &[P, P, F, P], &[P, F, P, P], &[P, P, P, P]],
        );
        assert_eq!(survival(&matrix, "me"), Some(0.5));
    }

    #[test]
    fn survival_is_none_when_nothing_measured_against_me() {
        // The only discriminating rival suite could not be built against
        // `me`: a missing measurement is not a survived-and-lost.
        let matrix = m(
            &["me", "x", "y", "z"],
            &[&[P, X, P, P], &[P, P, P, P], &[P, F, P, P], &[P, P, P, P]],
        );
        assert_eq!(
            suite_quality(&matrix, "x"),
            Some(SuiteQuality::Discriminating)
        );
        assert_eq!(survival(&matrix, "me"), None);
    }

    #[test]
    fn survival_is_none_for_unknown_arm() {
        let matrix = m(&["a"], &[&[P]]);
        assert_eq!(survival(&matrix, "nope"), None);
    }

    #[test]
    fn survival_zero_when_every_discriminating_suite_fails() {
        let matrix = m(&["me", "x", "y"], &[&[P, F, F], &[F, P, P], &[F, P, P]]);
        assert_eq!(survival(&matrix, "me"), Some(0.0));
    }

    #[test]
    fn discoveries_exclude_nocompile_and_are_sorted() {
        // d's suite fails on c and a, could not be built on b, passes e.
        let matrix = m(
            &["d", "c", "e", "b", "a"],
            &[
                &[P, P, P, P, P],
                &[F, P, P, P, P],
                &[P, P, P, P, P],
                &[X, P, P, P, P],
                &[F, P, P, P, P],
            ],
        );
        assert_eq!(
            discoveries(&matrix, "d"),
            vec!["a".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn self_pair_does_not_inflate_discoveries() {
        // d's suite fails on d itself: the self-pair must not be counted.
        let matrix = m(&["d", "e"], &[&[F, P], &[P, P]]);
        assert_eq!(discoveries(&matrix, "d"), Vec::<String>::new());
    }

    #[test]
    fn discoveries_are_empty_for_unknown_arm() {
        let matrix = m(&["a"], &[&[P]]);
        assert_eq!(discoveries(&matrix, "nope"), Vec::<String>::new());
    }

    #[test]
    fn get_maps_implementations_to_rows_and_suites_to_columns() {
        // Asymmetric on purpose: a's implementation fails b's suite, but b's
        // implementation passes a's.
        let matrix = m(&["a", "b"], &[&[P, F], &[P, P]]);
        assert_eq!(matrix.get("a", "b"), Some(F));
        assert_eq!(matrix.get("b", "a"), Some(P));
        assert_eq!(matrix.get("a", "a"), Some(P));
    }

    #[test]
    fn get_is_none_for_unknown_names() {
        let matrix = m(&["a", "b"], &[&[P, P], &[P, P]]);
        assert_eq!(matrix.get("a", "nope"), None);
        assert_eq!(matrix.get("nope", "a"), None);
        assert_eq!(matrix.get("nope", "also-nope"), None);
    }

    #[test]
    fn new_rejects_a_length_mismatch() {
        assert!(Matrix::new(vec!["a".into(), "b".into()], vec![P, P, P]).is_err());
        assert!(Matrix::new(vec!["a".into(), "b".into()], vec![P, P, P, P, P]).is_err());
        assert!(Matrix::new(vec!["a".into(), "b".into()], vec![P, P, P, P]).is_ok());
    }

    #[test]
    fn new_rejects_duplicate_arm_names() {
        assert!(Matrix::new(vec!["a".into(), "a".into()], vec![P, P, P, P]).is_err());
        assert!(Matrix::new(vec!["a".into(), "b".into(), "a".into()], vec![P; 9]).is_err());
    }

    #[test]
    fn arms_round_trip_in_matrix_order() {
        let matrix = m(&["z", "a"], &[&[P, P], &[P, P]]);
        assert_eq!(matrix.arms(), &["z".to_string(), "a".to_string()]);
    }

    #[test]
    fn one_fail_makes_the_shape_discriminating() {
        // Three arms, exactly one cross failure: ordinary disagreement.
        let matrix = m(&["a", "b", "c"], &[&[P, F, P], &[P, P, P], &[P, P, P]]);
        assert_eq!(shape(&matrix), Shape::Discriminating);
    }

    #[test]
    fn offdiagonal_nocompile_blocks_a_partition() {
        // Two clean camps on four arms, except one cross-camp cell was never
        // measured: "fails across" cannot be affirmed, so the shape is not
        // SpecAmbiguous.
        let matrix = m(
            &["a", "b", "c", "d"],
            &[&[P, P, X, F], &[P, P, F, F], &[F, F, P, P], &[F, F, P, P]],
        );
        assert_eq!(shape(&matrix), Shape::Discriminating);
    }

    // ---- Rule 1: Consensus reports the arms, sorted, de-duplicated --------

    #[test]
    fn rule1_all_pass_yields_consensus_with_every_arm() {
        let matrix = m(
            &["a", "b", "c", "d"],
            &[
                &[P, P, P, P],
                &[P, P, P, P],
                &[P, P, P, P],
                &[P, P, P, P],
            ],
        );
        assert_eq!(
            shape(&matrix),
            Shape::Consensus {
                arms: vec![
                    "a".to_string(),
                    "b".to_string(),
                    "c".to_string(),
                    "d".to_string()
                ]
            }
        );
    }

    #[test]
    fn rule1_arms_are_sorted_regardless_of_matrix_order() {
        // Arms supplied out of order must come back byte-sorted, unfiltered
        // and in full: every cell passed, so there is no status to select by.
        let matrix = m(
            &["delta", "alpha", "charlie", "bravo"],
            &[
                &[P, P, P, P],
                &[P, P, P, P],
                &[P, P, P, P],
                &[P, P, P, P],
            ],
        );
        assert_eq!(
            shape(&matrix),
            Shape::Consensus {
                arms: vec![
                    "alpha".to_string(),
                    "bravo".to_string(),
                    "charlie".to_string(),
                    "delta".to_string()
                ]
            }
        );
    }

    #[test]
    fn rule1_arms_sorted_by_byte_order() {
        // Byte order places every uppercase letter before any lowercase one.
        let matrix = m(
            &["b", "A", "a", "B"],
            &[
                &[P, P, P, P],
                &[P, P, P, P],
                &[P, P, P, P],
                &[P, P, P, P],
            ],
        );
        assert_eq!(
            shape(&matrix),
            Shape::Consensus {
                arms: vec![
                    "A".to_string(),
                    "B".to_string(),
                    "a".to_string(),
                    "b".to_string()
                ]
            }
        );
    }

    // ---- Rule 2: Void still runs first and still wins ---------------------

    #[test]
    fn rule2_broken_diagonal_voids_an_all_pass_off_diagonal_matrix() {
        // Every off-diagonal cell passes, but arm b's own suite failed on b's
        // own code: a broken instrument's agreement is worthless, so Void.
        let matrix = m(&["a", "b", "c"], &[&[P, P, P], &[P, F, P], &[P, P, P]]);
        assert_eq!(
            shape(&matrix),
            Shape::Void {
                broken: vec!["b".to_string()]
            }
        );
    }

    #[test]
    fn rule2_void_wins_over_consensus_on_four_arms() {
        // Diagonal cell for `c` is NoCompile (not Pass), every off-diagonal
        // cell passes: the broken-instrument check still outranks Consensus.
        let matrix = m(
            &["a", "b", "c", "d"],
            &[
                &[P, P, P, P],
                &[P, P, P, P],
                &[P, P, X, P],
                &[P, P, P, P],
            ],
        );
        assert_eq!(
            shape(&matrix),
            Shape::Void {
                broken: vec!["c".to_string()]
            }
        );
    }

    // ---- Rule 3: one off-diagonal Fail prevents Consensus -----------------

    #[test]
    fn rule3_single_offdiagonal_fail_prevents_consensus() {
        let matrix = m(
            &["a", "b", "c"],
            &[&[P, P, P], &[P, P, P], &[F, P, P]],
        );
        assert_eq!(shape(&matrix), Shape::Discriminating);
    }

    #[test]
    fn rule3_single_offdiagonal_fail_prevents_consensus_on_five_arms() {
        // The lone failure sits far from the diagonal; everything else passes.
        let matrix = m(
            &["a", "b", "c", "d", "e"],
            &[
                &[P, P, P, P, P],
                &[P, P, P, P, P],
                &[P, P, P, P, P],
                &[P, P, P, P, P],
                &[P, F, P, P, P],
            ],
        );
        assert_eq!(shape(&matrix), Shape::Discriminating);
    }

    // ---- Rule 4: an off-diagonal NoCompile prevents Consensus -------------

    #[test]
    fn rule4_offdiagonal_nocompile_prevents_consensus() {
        let matrix = m(&["a", "b"], &[&[P, X], &[P, P]]);
        let s = shape(&matrix);
        assert!(
            !matches!(s, Shape::Consensus { .. }),
            "a NoCompile pair must not read as consensus: {s:?}"
        );
        assert_eq!(s, Shape::Discriminating);
    }

    #[test]
    fn rule4_no_offdiagonal_fail_still_not_consensus_under_nocompile() {
        // Diagonal all-Pass, every other cell Pass *or* NoCompile, at least
        // one NoCompile: agreement was never measured on that pair.
        let matrix = m(
            &["a", "b", "c"],
            &[&[P, P, X], &[P, P, P], &[P, P, P]],
        );
        let s = shape(&matrix);
        assert!(
            !matches!(s, Shape::Consensus { .. }),
            "expected non-Consensus, got {s:?}"
        );
        assert_eq!(s, Shape::Discriminating);
    }

    // ---- Rule 5: is_discriminating, exhaustive over Shape -----------------

    #[test]
    fn rule5_consensus_is_not_discriminating() {
        let consensus = Shape::Consensus {
            arms: vec!["a".to_string(), "b".to_string()],
        };
        assert!(!is_discriminating(&consensus));
    }

    #[test]
    fn rule5_void_is_not_discriminating() {
        assert!(!is_discriminating(&Shape::Void {
            broken: vec!["a".to_string()]
        }));
        assert!(!is_discriminating(&Shape::Void { broken: vec![] }));
    }

    #[test]
    fn rule5_other_shapes_are_discriminating() {
        assert!(is_discriminating(&Shape::Discriminating));
        assert!(is_discriminating(&Shape::SpecAmbiguous {
            camps: vec![
                vec!["a".to_string(), "b".to_string()],
                vec!["c".to_string(), "d".to_string()]
            ]
        }));
    }

    #[test]
    fn rule5_every_shape_the_classifier_can_emit_matches_its_own_reading() {
        // A real matrix per shape, run through the classifier, then checked.
        let consensus = m(&["a", "b"], &[&[P, P], &[P, P]]);
        let void_ = m(&["a", "b"], &[&[F, P], &[P, P]]);
        let discriminating = m(&["a", "b"], &[&[P, F], &[F, P]]);
        let ambiguous = partition();
        assert!(!is_discriminating(&shape(&consensus)));
        assert!(!is_discriminating(&shape(&void_)));
        assert!(is_discriminating(&shape(&discriminating)));
        assert!(is_discriminating(&shape(&ambiguous)));
        assert!(matches!(shape(&ambiguous), Shape::SpecAmbiguous { .. }));
    }

    // ---- Boundaries --------------------------------------------------------

    #[test]
    fn boundary_one_arm_matrix_is_vacuous_consensus() {
        let matrix = m(&["solo"], &[&[P]]);
        let s = shape(&matrix);
        assert_eq!(
            s,
            Shape::Consensus {
                arms: vec!["solo".to_string()]
            }
        );
        assert!(!is_discriminating(&s), "one arm agreeing with itself");
    }

    #[test]
    fn boundary_empty_matrix_keeps_its_reading_with_empty_arms() {
        // Whatever the classifier returned for an empty matrix before stays;
        // today that is Consensus, now carrying an empty arm list.
        let matrix = Matrix::new(Vec::new(), Vec::new()).expect("empty matrix");
        let s = shape(&matrix);
        assert!(
            matches!(s, Shape::Consensus { ref arms } if arms.is_empty()),
            "empty matrix reading changed: {s:?}"
        );
    }

    #[test]
    fn boundary_two_by_two_all_pass_is_consensus_with_two_arms() {
        let matrix = m(&["b", "a"], &[&[P, P], &[P, P]]);
        assert_eq!(
            shape(&matrix),
            Shape::Consensus {
                arms: vec!["a".to_string(), "b".to_string()]
            }
        );
    }

    #[test]
    fn boundary_all_nocompile_off_diagonal_is_not_consensus() {
        // Four arms, diagonal passing, every cross cell unmeasured: they did
        // not agree, they failed to be comparable.
        let matrix = m(
            &["a", "b", "c", "d"],
            &[
                &[P, X, X, X],
                &[X, P, X, X],
                &[X, X, P, X],
                &[X, X, X, P],
            ],
        );
        let s = shape(&matrix);
        assert!(
            !matches!(s, Shape::Consensus { .. }),
            "all-NoCompile field read as consensus: {s:?}"
        );
        assert_eq!(s, Shape::Discriminating);
    }
}
