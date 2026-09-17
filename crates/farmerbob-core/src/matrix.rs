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
    /// Every cell passes.
    Consensus,
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
        return Shape::Consensus;
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
    use super::{Cell, Matrix, Shape, SuiteQuality, discoveries, shape, suite_quality, survival};

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
        assert_eq!(shape(&matrix), Shape::Consensus);
    }

    #[test]
    fn one_nocompile_prevents_consensus() {
        let matrix = m(&["a", "b", "c"], &[&[P, P, P], &[P, P, X], &[P, P, P]]);
        assert_eq!(shape(&matrix), Shape::Discriminating);
    }

    #[test]
    fn single_arm_consensus() {
        let matrix = m(&["solo"], &[&[P]]);
        assert_eq!(shape(&matrix), Shape::Consensus);
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
}
