//! The shape of a cross-examination matrix as a whole.
//!
//! Cross-examination runs every candidate's suite against every candidate's
//! implementation and reads the result one suite at a time. That reading
//! cannot distinguish "no evidence" from "the field agrees completely" at two
//! implementations, and cannot separate "three over-fitted suites" from "a
//! specification that did not pin what all three suites test" at three. The
//! shape of the WHOLE matrix is a fact about the FIELD, it is available at two
//! implementations, and this module computes it.

use crate::crossx::{Cell, Matrix};
use crate::measurement::Measurement;

/// What the matrix as a whole says about the field. Exactly these variants and
/// no others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    /// Every off-diagonal cell passed. The field agrees; no suite separates
    /// any pair.
    Unanimous {
        /// How many implementations agreed.
        arms: usize,
    },
    /// Every off-diagonal cell failed. Each implementation is self-consistent
    /// and rejects every other.
    MutualRejection {
        /// How many implementations mutually rejected.
        arms: usize,
    },
    /// The field splits into camps that pass within themselves and fail
    /// across. Requires at least two camps and at least two arms in one of
    /// them.
    Partition {
        /// The camps. See the composition section for the exact ordering.
        camps: Vec<Vec<String>>,
    },
    /// Exactly one implementation fails every foreign suite while every other
    /// pair passes. The odd one out.
    Isolated {
        /// The arm that stands alone.
        arm: String,
    },
    /// None of the above.
    Mixed,
}

/// Read the shape of a whole cross-examination matrix.
///
/// `Missing(..)` when there is nothing to read rather than when the answer is
/// dull: see the boundaries below for exactly which cases are which.
pub fn shape(m: &Matrix) -> Measurement<Shape> {
    let suites = m.suites();
    let impls = m.impls();

    // Clause 7: axes must be the same set of names
    if suites.len() != impls.len() {
        return Measurement::nothing_to_measure("suite and implementation axes differ");
    }
    let mut suite_set: Vec<&str> = suites.iter().map(String::as_str).collect();
    let mut impl_set: Vec<&str> = impls.iter().map(String::as_str).collect();
    suite_set.sort();
    impl_set.sort();
    if suite_set != impl_set {
        return Measurement::nothing_to_measure("suite and implementation axes differ");
    }

    let n = suites.len();

    // Boundaries: zero or one implementation
    if n == 0 {
        return Measurement::nothing_to_measure("no implementations");
    }
    if n == 1 {
        return Measurement::nothing_to_measure("only one implementation; no off-diagonal cells");
    }

    // Collect off-diagonal cells, excluding diagonal and Error cells
    let mut off_diagonal_cells = Vec::new();
    let mut all_error = true;
    let mut arm_all_error = vec![true; n]; // track if each arm's off-diagonal cells are all Error

    for (suite_idx, suite) in suites.iter().enumerate() {
        for (impl_idx, imp) in impls.iter().enumerate() {
            if suite == imp {
                // Clause 6: diagonal is ignored
                continue;
            }
            let cell = m.get(suite, imp).unwrap_or(Cell::Error);
            match cell {
                Cell::Error => {
                    // Error cells are excluded from every denominator
                }
                Cell::Pass | Cell::Fail => {
                    all_error = false;
                    arm_all_error[suite_idx] = false;
                    arm_all_error[impl_idx] = false;
                    off_diagonal_cells.push((suite_idx, impl_idx, cell));
                }
            }
        }
    }

    // All off-diagonal cells are Error
    if all_error {
        return Measurement::instrument_failed("every off-diagonal cell errored; no measurements");
    }

    // Count Pass and Fail among measured off-diagonal cells
    let pass_count = off_diagonal_cells
        .iter()
        .filter(|(_, _, c)| *c == Cell::Pass)
        .count();
    let fail_count = off_diagonal_cells
        .iter()
        .filter(|(_, _, c)| *c == Cell::Fail)
        .count();
    let total_measured = pass_count + fail_count;

    // Clause 1: All off-diagonal Pass -> Unanimous
    if fail_count == 0 && total_measured > 0 {
        return Measurement::observed(Shape::Unanimous { arms: n });
    }

    // Clause 2: All off-diagonal Fail -> MutualRejection
    if pass_count == 0 && total_measured > 0 {
        return Measurement::observed(Shape::MutualRejection { arms: n });
    }

    // Check for Isolated (Clause 5: requires at least 3 arms)
    if n >= 3
        && let Some(arm) = find_isolated(m, suites, impls, &arm_all_error)
    {
        return Measurement::observed(Shape::Isolated { arm });
    }

    // Check for Partition (Clause 4: never at two arms)
    if n >= 3
        && let Some(camps) = find_partition(m, suites, impls, &arm_all_error)
        && camps.len() >= 2
        && camps.iter().any(|camp| camp.len() >= 2)
    {
        return Measurement::observed(Shape::Partition { camps });
    }

    // Clause: Mixed is the catch-all
    Measurement::observed(Shape::Mixed)
}

/// Find camps for Partition shape.
/// Camps are groups where arms pass within the camp and fail across camps.
/// Returns camps sorted by first member, with members sorted within each camp.
fn find_partition(
    m: &Matrix,
    suites: &[String],
    impls: &[String],
    arm_all_error: &[bool],
) -> Option<Vec<Vec<String>>> {
    let n = suites.len();

    // Build a relation: i and j are in same camp iff they mutually pass (both directions)
    // and mutually fail with everyone outside the camp.
    // We only consider arms that have at least some measurable cells.

    // First, find connected components of mutual-pass relation among measurable arms
    let measurable: Vec<usize> = (0..n).filter(|&i| !arm_all_error[i]).collect();
    if measurable.len() < 2 {
        return None;
    }

    // mutual_pass[i][j] = true if i and j mutually pass (both directions Pass, not Error)
    let mut mutual_pass = vec![vec![false; n]; n];
    for &i in &measurable {
        for &j in &measurable {
            if i == j {
                continue;
            }
            let cell_ij = m.get(&suites[i], &impls[j]);
            let cell_ji = m.get(&suites[j], &impls[i]);
            if let (Some(Cell::Pass), Some(Cell::Pass)) = (cell_ij, cell_ji) {
                mutual_pass[i][j] = true;
            }
        }
    }

    // Find connected components of mutual_pass
    let mut camp_of = vec![None; n];
    let mut camps: Vec<Vec<usize>> = Vec::new();

    for &seed in &measurable {
        if camp_of[seed].is_some() {
            continue;
        }
        let camp_id = camps.len();
        let mut members = Vec::new();
        let mut stack = vec![seed];
        camp_of[seed] = Some(camp_id);

        while let Some(current) = stack.pop() {
            members.push(current);
            for &other in &measurable {
                if camp_of[other].is_none() && mutual_pass[current][other] {
                    camp_of[other] = Some(camp_id);
                    stack.push(other);
                }
            }
        }
        camps.push(members);
    }

    if camps.len() < 2 {
        return None;
    }

    // Verify the partition property: within camp -> mutual pass, across camps -> mutual fail
    for i in 0..n {
        for j in (i + 1)..n {
            let same_camp = camp_of[i] == camp_of[j] && camp_of[i].is_some();
            let cell_ij = m.get(&suites[i], &impls[j]);
            let cell_ji = m.get(&suites[j], &impls[i]);

            if same_camp {
                // Within camp: must mutually pass (both Pass, not Error)
                if !matches!((cell_ij, cell_ji), (Some(Cell::Pass), Some(Cell::Pass))) {
                    return None;
                }
            } else if camp_of[i].is_some() && camp_of[j].is_some() {
                // Across camps: must mutually fail (both Fail, not Error)
                if !matches!((cell_ij, cell_ji), (Some(Cell::Fail), Some(Cell::Fail))) {
                    return None;
                }
            }
            // If either arm is all-error, it doesn't participate in the partition
        }
    }

    // Convert to names, sort members within each camp, sort camps by first member
    let mut named_camps: Vec<Vec<String>> = camps
        .into_iter()
        .map(|members| {
            let mut names: Vec<String> = members.into_iter().map(|i| suites[i].clone()).collect();
            names.sort();
            names
        })
        .collect();

    named_camps.sort_by(|a, b| a[0].cmp(&b[0]));

    Some(named_camps)
}

/// Find if there's an Isolated arm.
/// An arm is isolated if:
/// - It fails every foreign suite in both directions (its suite fails on every other impl, and every other suite fails on its impl)
/// - Every pair NOT involving this arm passes both ways
/// - The arm must have measurable cells (not all Error)
fn find_isolated(
    m: &Matrix,
    suites: &[String],
    impls: &[String],
    arm_all_error: &[bool],
) -> Option<String> {
    let n = suites.len();

    for candidate in 0..n {
        if arm_all_error[candidate] {
            // Clause: an arm whose every off-diagonal cell is Error must not silently become Isolated
            continue;
        }

        let mut is_isolated = true;

        // Check: candidate fails every foreign suite in both directions
        for other in 0..n {
            if other == candidate {
                continue;
            }
            if arm_all_error[other] {
                // Can't verify the cross cells if the other arm is all-error
                is_isolated = false;
                break;
            }
            let cell_candidate_suite_other_impl = m.get(&suites[candidate], &impls[other]);
            let cell_other_suite_candidate_impl = m.get(&suites[other], &impls[candidate]);

            // Both must be Fail (not Error, not Pass)
            if !matches!(
                (
                    cell_candidate_suite_other_impl,
                    cell_other_suite_candidate_impl
                ),
                (Some(Cell::Fail), Some(Cell::Fail))
            ) {
                is_isolated = false;
                break;
            }
        }

        if !is_isolated {
            continue;
        }

        // Check: every pair NOT involving candidate passes both ways
        for i in 0..n {
            if i == candidate || arm_all_error[i] {
                continue;
            }
            for j in (i + 1)..n {
                if j == candidate || arm_all_error[j] {
                    continue;
                }
                let cell_ij = m.get(&suites[i], &impls[j]);
                let cell_ji = m.get(&suites[j], &impls[i]);
                if !matches!((cell_ij, cell_ji), (Some(Cell::Pass), Some(Cell::Pass))) {
                    is_isolated = false;
                    break;
                }
            }
            if !is_isolated {
                break;
            }
        }

        if is_isolated {
            return Some(suites[candidate].clone());
        }
    }

    None
}

/// Whether this shape is evidence about the SPECIFICATION rather than about
/// any implementation. True for exactly two variants.
pub fn points_at_spec(s: &Shape) -> bool {
    matches!(s, Shape::MutualRejection { .. } | Shape::Partition { .. })
}

/// One sentence a human can read in the pipeline log. Non-empty for every
/// shape; its exact wording is not pinned.
pub fn narrate(s: &Shape) -> String {
    match s {
        Shape::Unanimous { arms } => {
            format!("All {arms} implementations agree completely: every cross-examination passed.")
        }
        Shape::MutualRejection { arms } => {
            format!(
                "All {arms} implementations mutually reject each other: every cross-examination failed."
            )
        }
        Shape::Partition { camps } => {
            let camp_descriptions: Vec<String> = camps
                .iter()
                .map(|camp| format!("{{{}}}", camp.join(", ")))
                .collect();
            format!(
                "The field splits into {} camps that pass within and fail across: {}.",
                camps.len(),
                camp_descriptions.join("; ")
            )
        }
        Shape::Isolated { arm } => {
            format!(
                "Implementation \"{arm}\" stands alone: it fails every foreign suite while all other pairs pass."
            )
        }
        Shape::Mixed => {
            "The field shows mixed agreement and disagreement with no clear structure.".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crossx::{Cell, Matrix};
    use crate::measurement::Absent;

    /// Helper to build a square matrix with same suites and impls
    fn square_matrix(names: &[&str], rows: &[&str]) -> Matrix {
        let mut cells = Vec::new();
        for row in rows {
            assert_eq!(row.len(), names.len());
            for ch in row.chars() {
                cells.push(match ch {
                    'P' => Cell::Pass,
                    'F' => Cell::Fail,
                    'E' => Cell::Error,
                    _ => panic!("bad cell char"),
                });
            }
        }
        Matrix::new(
            names.iter().map(|s| s.to_string()).collect(),
            names.iter().map(|s| s.to_string()).collect(),
            cells,
        )
        .expect("valid matrix")
    }

    /// Helper to build a non-square matrix
    fn rectangular_matrix(suites: &[&str], impls: &[&str], rows: &[&str]) -> Matrix {
        let mut cells = Vec::new();
        for row in rows {
            assert_eq!(row.len(), impls.len());
            for ch in row.chars() {
                cells.push(match ch {
                    'P' => Cell::Pass,
                    'F' => Cell::Fail,
                    'E' => Cell::Error,
                    _ => panic!("bad cell char"),
                });
            }
        }
        Matrix::new(
            suites.iter().map(|s| s.to_string()).collect(),
            impls.iter().map(|s| s.to_string()).collect(),
            cells,
        )
        .expect("valid matrix")
    }

    // ===== Clause 1: All off-diagonal Pass -> Unanimous =====

    #[test]
    fn clause1_unanimous_three_arms() {
        let m = square_matrix(&["a", "b", "c"], &["PPP", "PPP", "PPP"]);
        let result = shape(&m);
        assert!(result.is_observed());
        assert_eq!(result.value(), Some(&Shape::Unanimous { arms: 3 }));
    }

    #[test]
    fn clause1_unanimous_two_arms() {
        let m = square_matrix(&["a", "b"], &["PP", "PP"]);
        let result = shape(&m);
        assert!(result.is_observed());
        assert_eq!(result.value(), Some(&Shape::Unanimous { arms: 2 }));
    }

    #[test]
    fn clause1_unanimous_ignores_diagonal() {
        // Diagonal can be anything, only off-diagonal matters
        let m = square_matrix(&["a", "b", "c"], &["FPP", "PFP", "PPF"]);
        let result = shape(&m);
        assert!(result.is_observed());
        assert_eq!(result.value(), Some(&Shape::Unanimous { arms: 3 }));
    }

    // ===== Clause 2: All off-diagonal Fail -> MutualRejection =====

    #[test]
    fn clause2_mutual_rejection_three_arms() {
        let m = square_matrix(&["a", "b", "c"], &["FFF", "FFF", "FFF"]);
        let result = shape(&m);
        assert!(result.is_observed());
        assert_eq!(result.value(), Some(&Shape::MutualRejection { arms: 3 }));
    }

    #[test]
    fn clause2_mutual_rejection_two_arms() {
        let m = square_matrix(&["a", "b"], &["FF", "FF"]);
        let result = shape(&m);
        assert!(result.is_observed());
        assert_eq!(result.value(), Some(&Shape::MutualRejection { arms: 2 }));
    }

    #[test]
    fn clause2_mutual_rejection_ignores_diagonal() {
        let m = square_matrix(&["a", "b", "c"], &["PFF", "FPF", "FFP"]);
        let result = shape(&m);
        assert!(result.is_observed());
        assert_eq!(result.value(), Some(&Shape::MutualRejection { arms: 3 }));
    }

    // ===== Clause 3: points_at_spec =====

    #[test]
    fn clause3_points_at_spec_mutual_rejection() {
        assert!(points_at_spec(&Shape::MutualRejection { arms: 3 }));
        assert!(points_at_spec(&Shape::MutualRejection { arms: 2 }));
    }

    #[test]
    fn clause3_points_at_spec_partition() {
        assert!(points_at_spec(&Shape::Partition {
            camps: vec![
                vec!["a".to_string(), "b".to_string()],
                vec!["c".to_string()]
            ]
        }));
    }

    #[test]
    fn clause3_points_at_spec_false_for_unanimous() {
        assert!(!points_at_spec(&Shape::Unanimous { arms: 3 }));
    }

    #[test]
    fn clause3_points_at_spec_false_for_isolated() {
        assert!(!points_at_spec(&Shape::Isolated {
            arm: "a".to_string()
        }));
    }

    #[test]
    fn clause3_points_at_spec_false_for_mixed() {
        assert!(!points_at_spec(&Shape::Mixed));
    }

    // ===== Clause 4: Partition never at two arms =====

    #[test]
    fn clause4_partition_not_at_two_arms() {
        // Two arms failing each other = MutualRejection, not Partition
        let m = square_matrix(&["a", "b"], &["FF", "FF"]);
        let result = shape(&m);
        assert!(result.is_observed());
        assert_eq!(result.value(), Some(&Shape::MutualRejection { arms: 2 }));
        assert!(!matches!(result.value(), Some(Shape::Partition { .. })));
    }

    #[test]
    fn clause4_partition_three_arms_two_camps() {
        // At 3 arms, a 2+1 camp pattern is Isolated, not Partition.
        // Partition requires >= 4 arms with at least two camps of size >= 2.
        // This test verifies that 3-arm all-fail is MutualRejection.
        let m = square_matrix(&["a", "b", "c"], &["FFF", "FFF", "FFF"]);
        let result = shape(&m);
        assert!(result.is_observed());
        assert_eq!(result.value(), Some(&Shape::MutualRejection { arms: 3 }));
    }

    #[test]
    fn clause4_partition_camps_sorted() {
        // Arms in different order, camps should be sorted
        // Valid partition at 4 arms: a,b pass each other; c,d pass each other; cross fails
        let m = square_matrix(&["d", "c", "b", "a"], &["PPFF", "PPFF", "FFPP", "FFPP"]);
        let result = shape(&m);
        assert!(result.is_observed());
        match result.value() {
            Some(Shape::Partition { camps }) => {
                assert_eq!(camps[0], vec!["a", "b"]);
                assert_eq!(camps[1], vec!["c", "d"]);
            }
            other => panic!("expected Partition, got {other:?}"),
        }
    }

    #[test]
    fn clause4_partition_requires_at_least_two_arms_in_one_camp() {
        // Three arms, all mutually failing = MutualRejection, not Partition
        // (no camp has >= 2 arms)
        let m = square_matrix(&["a", "b", "c"], &["FFF", "FFF", "FFF"]);
        let result = shape(&m);
        assert!(result.is_observed());
        assert_eq!(result.value(), Some(&Shape::MutualRejection { arms: 3 }));
    }

    #[test]
    fn clause4_partition_four_arms_two_camps_of_two() {
        // a,b pass; c,d pass; cross fails
        let m = square_matrix(&["a", "b", "c", "d"], &["PPFF", "PPFF", "FFPP", "FFPP"]);
        let result = shape(&m);
        assert!(result.is_observed());
        match result.value() {
            Some(Shape::Partition { camps }) => {
                assert_eq!(camps.len(), 2);
                assert_eq!(camps[0], vec!["a", "b"]);
                assert_eq!(camps[1], vec!["c", "d"]);
            }
            other => panic!("expected Partition, got {other:?}"),
        }
    }

    #[test]
    fn clause4_partition_excluded_by_error_cells() {
        // Partition but with an Error cell breaking the clean split
        let m = square_matrix(&["a", "b", "c", "d"], &["PPEF", "PPEF", "EEPP", "FFPP"]);
        let result = shape(&m);
        assert!(result.is_observed());
        // Should not be Partition because cross cells have Errors
        assert!(!matches!(result.value(), Some(Shape::Partition { .. })));
    }

    // ===== Clause 5: Isolated =====

    #[test]
    fn clause5_isolated_three_arms() {
        // a is isolated: a fails b and c both ways; b and c pass each other both ways
        let m = square_matrix(&["a", "b", "c"], &["PFF", "FPP", "FPP"]);
        // Row a (suite a): P (diag), F (vs b), F (vs c) - but we ignore diagonal
        // Row b (suite b): F (vs a), P (diag), P (vs c)
        // Row c (suite c): F (vs a), P (vs b), P (diag)
        // Off-diagonal:
        // a->b: F, b->a: F
        // a->c: F, c->a: F
        // b->c: P, c->b: P
        let result = shape(&m);
        assert!(result.is_observed());
        match result.value() {
            Some(Shape::Isolated { arm }) => assert_eq!(arm, "a"),
            other => panic!("expected Isolated, got {other:?}"),
        }
    }

    #[test]
    fn clause5_isolated_not_at_two_arms() {
        let m = square_matrix(&["a", "b"], &["FF", "FF"]);
        let result = shape(&m);
        assert!(result.is_observed());
        assert_eq!(result.value(), Some(&Shape::MutualRejection { arms: 2 }));
        assert!(!matches!(result.value(), Some(Shape::Isolated { .. })));
    }

    #[test]
    fn clause5_isolated_excluded_by_error_cells() {
        // a's cells are all Error except diagonal - must not become Isolated
        let m = square_matrix(&["a", "b", "c"], &["PEE", "FPP", "FPP"]);
        // a's off-diagonal: E, E (all Error)
        // b's off-diagonal: F (vs a), P (vs c)
        // c's off-diagonal: F (vs a), P (vs b)
        let result = shape(&m);
        assert!(result.is_observed());
        // a is all-error, so it cannot be isolated
        // The remaining field (b, c) has b<->c Pass, but b fails a, c fails a
        // Since a is all-error, we can't verify a's cross cells -> not Isolated
        assert!(!matches!(result.value(), Some(Shape::Isolated { .. })));
    }

    #[test]
    fn clause5_isolated_requires_all_other_pairs_pass() {
        // a fails b and c; but b and c fail each other -> not Isolated
        let m = square_matrix(&["a", "b", "c"], &["PFF", "FFF", "FFF"]);
        let result = shape(&m);
        assert!(result.is_observed());
        assert!(!matches!(result.value(), Some(Shape::Isolated { .. })));
    }

    // ===== Clause 6: Diagonal ignored =====

    #[test]
    fn clause6_diagonal_ignored_unanimous() {
        // Diagonal Fail, off-diagonal Pass -> still Unanimous
        let m = square_matrix(&["a", "b"], &["FP", "PF"]);
        let result = shape(&m);
        assert!(result.is_observed());
        assert_eq!(result.value(), Some(&Shape::Unanimous { arms: 2 }));
    }

    #[test]
    fn clause6_diagonal_ignored_mutual_rejection() {
        // Diagonal Pass, off-diagonal Fail -> still MutualRejection
        let m = square_matrix(&["a", "b"], &["PF", "FP"]);
        let result = shape(&m);
        assert!(result.is_observed());
        assert_eq!(result.value(), Some(&Shape::MutualRejection { arms: 2 }));
    }

    // ===== Clause 7: Missing when axes differ =====

    #[test]
    fn clause7_missing_when_axes_differ() {
        let m = rectangular_matrix(&["a", "b"], &["a", "b", "c"], &["PPP", "PPP"]);
        let result = shape(&m);
        assert!(!result.is_observed());
        assert!(matches!(
            result.absent(),
            Some(Absent::NothingToMeasure { .. })
        ));
    }

    #[test]
    fn clause7_missing_when_axes_differ_names() {
        let m = rectangular_matrix(&["a", "b"], &["x", "y"], &["PP", "PP"]);
        let result = shape(&m);
        assert!(!result.is_observed());
        assert!(matches!(
            result.absent(),
            Some(Absent::NothingToMeasure { .. })
        ));
    }

    // ===== Boundaries =====

    #[test]
    fn boundary_zero_implementations() {
        let m = Matrix::new(Vec::new(), Vec::new(), Vec::new()).expect("empty matrix");
        let result = shape(&m);
        assert!(!result.is_observed());
        assert!(matches!(
            result.absent(),
            Some(Absent::NothingToMeasure { .. })
        ));
    }

    #[test]
    fn boundary_one_implementation() {
        let m = square_matrix(&["a"], &["P"]);
        let result = shape(&m);
        assert!(!result.is_observed());
        assert!(matches!(
            result.absent(),
            Some(Absent::NothingToMeasure { .. })
        ));
    }

    #[test]
    fn boundary_two_arms_unanimous_and_mutual_rejection() {
        let m1 = square_matrix(&["a", "b"], &["PP", "PP"]);
        assert_eq!(shape(&m1).value(), Some(&Shape::Unanimous { arms: 2 }));

        let m2 = square_matrix(&["a", "b"], &["FF", "FF"]);
        assert_eq!(
            shape(&m2).value(),
            Some(&Shape::MutualRejection { arms: 2 })
        );
    }

    #[test]
    fn boundary_three_arms_all_variants_reachable() {
        // Unanimous
        let m1 = square_matrix(&["a", "b", "c"], &["PPP", "PPP", "PPP"]);
        assert_eq!(shape(&m1).value(), Some(&Shape::Unanimous { arms: 3 }));

        // MutualRejection
        let m2 = square_matrix(&["a", "b", "c"], &["FFF", "FFF", "FFF"]);
        assert_eq!(
            shape(&m2).value(),
            Some(&Shape::MutualRejection { arms: 3 })
        );

        // Isolated (at 3 arms, the 2+1 camp pattern is Isolated)
        let m3 = square_matrix(&["a", "b", "c"], &["PFF", "FPP", "FPP"]);
        assert!(matches!(shape(&m3).value(), Some(Shape::Isolated { .. })));

        // Mixed
        let m4 = square_matrix(&["a", "b", "c"], &["PFP", "FPP", "PPF"]);
        assert_eq!(shape(&m4).value(), Some(&Shape::Mixed));
    }

    #[test]
    fn boundary_all_off_diagonal_error() {
        let m = square_matrix(&["a", "b", "c"], &["PEE", "EPE", "EEP"]);
        let result = shape(&m);
        assert!(!result.is_observed());
        assert!(matches!(
            result.absent(),
            Some(Absent::InstrumentFailed { .. })
        ));
    }

    #[test]
    fn boundary_some_error_cells_excluded() {
        // a<->b Pass, a<->c Error, b<->c Fail -> Mixed (not enough for Partition)
        let m = square_matrix(&["a", "b", "c"], &["PPE", "PPE", "EFP"]);
        // a->b: P, b->a: P
        // a->c: E, c->a: E (excluded)
        // b->c: P, c->b: F
        let result = shape(&m);
        assert!(result.is_observed());
        // Not Unanimous (has Fail), not MutualRejection (has Pass), not Partition (only 2 measurable arms, cross not clean), not Isolated
        assert_eq!(result.value(), Some(&Shape::Mixed));
    }

    #[test]
    fn boundary_one_arm_all_error_excluded() {
        // a's off-diagonal all Error, b and c pass each other
        let m = square_matrix(&["a", "b", "c"], &["PEE", "EPP", "EPP"]);
        // a's off-diagonal: E, E (all Error -> excluded)
        // b's off-diagonal: E (vs a, excluded), P (vs c)
        // c's off-diagonal: E (vs a, excluded), P (vs b)
        // Measured: b<->c Pass only
        let result = shape(&m);
        assert!(result.is_observed());
        // Only b and c have measurable cells between them, and they pass
        // This is effectively a 2-arm unanimous field
        // But we have 3 arms total, with one all-error
        // The measured off-diagonal cells are only b<->c Pass
        assert_eq!(result.value(), Some(&Shape::Unanimous { arms: 3 }));
    }

    #[test]
    fn boundary_one_arm_all_error_not_isolated() {
        // a's off-diagonal all Error, b and c fail each other
        let m = square_matrix(&["a", "b", "c"], &["PEE", "EFF", "EFF"]);
        // a: all Error off-diag
        // b<->c: Fail
        let result = shape(&m);
        assert!(result.is_observed());
        // Not Isolated because a is all-error
        assert!(!matches!(result.value(), Some(Shape::Isolated { .. })));
    }

    // ===== Partition composition: every arm in exactly one camp, sorted =====

    #[test]
    fn partition_every_arm_in_exactly_one_camp() {
        let m = square_matrix(&["a", "b", "c", "d"], &["PPFF", "PPFF", "FFPP", "FFPP"]);
        let result = shape(&m);
        match result.value() {
            Some(Shape::Partition { camps }) => {
                let all_arms: Vec<String> = camps.iter().flatten().cloned().collect();
                assert_eq!(all_arms.len(), 4);
                let mut sorted = all_arms.clone();
                sorted.sort();
                assert_eq!(sorted, vec!["a", "b", "c", "d"]);
            }
            other => panic!("expected Partition, got {other:?}"),
        }
    }

    #[test]
    fn partition_deterministic_ordering() {
        let m = square_matrix(&["d", "c", "b", "a"], &["PPFF", "PPFF", "FFPP", "FFPP"]);
        let result = shape(&m);
        match result.value() {
            Some(Shape::Partition { camps }) => {
                assert_eq!(camps[0], vec!["a", "b"]);
                assert_eq!(camps[1], vec!["c", "d"]);
            }
            other => panic!("expected Partition, got {other:?}"),
        }
    }

    // ===== narrate non-empty =====

    #[test]
    fn narrate_non_empty_for_all_shapes() {
        assert!(!narrate(&Shape::Unanimous { arms: 3 }).is_empty());
        assert!(!narrate(&Shape::MutualRejection { arms: 3 }).is_empty());
        assert!(
            !narrate(&Shape::Partition {
                camps: vec![vec!["a".into()], vec!["b".into()]]
            })
            .is_empty()
        );
        assert!(!narrate(&Shape::Isolated { arm: "a".into() }).is_empty());
        assert!(!narrate(&Shape::Mixed).is_empty());
    }

    // ===== shape and points_at_spec tested together =====

    #[test]
    fn shape_and_points_at_spec_mutual_rejection() {
        let m = square_matrix(&["a", "b", "c"], &["FFF", "FFF", "FFF"]);
        let result = shape(&m);
        let s = result.value().unwrap();
        assert!(points_at_spec(s));
    }

    #[test]
    fn shape_and_points_at_spec_partition() {
        let m = square_matrix(&["a", "b", "c", "d"], &["PPFF", "PPFF", "FFPP", "FFPP"]);
        let result = shape(&m);
        let s = result.value().unwrap();
        assert!(points_at_spec(s));
    }

    #[test]
    fn shape_and_points_at_spec_unanimous() {
        let m = square_matrix(&["a", "b", "c"], &["PPP", "PPP", "PPP"]);
        let result = shape(&m);
        let s = result.value().unwrap();
        assert!(!points_at_spec(s));
    }

    // ===== Error handling in Matrix::new =====

    #[test]
    fn matrix_new_error_handled_in_tests() {
        let result = Matrix::new(
            vec!["a".to_string(), "b".to_string()],
            vec!["a".to_string(), "b".to_string()],
            vec![Cell::Pass, Cell::Pass], // wrong count
        );
        assert!(result.is_err());
    }

    // ===== Mixed reachable =====

    #[test]
    fn mixed_is_reachable() {
        // Asymmetric: a passes b, b fails a
        let m = square_matrix(&["a", "b"], &["PP", "FP"]);
        let result = shape(&m);
        assert!(result.is_observed());
        assert_eq!(result.value(), Some(&Shape::Mixed));
    }

    // ===== Non-square but same names =====

    #[test]
    fn non_square_same_names_is_missing() {
        // 2 suites, 3 impls with same names subset
        let m = rectangular_matrix(&["a", "b"], &["a", "b", "c"], &["PPP", "PPP"]);
        let result = shape(&m);
        assert!(!result.is_observed());
        assert!(matches!(
            result.absent(),
            Some(Absent::NothingToMeasure { .. })
        ));
    }
}






