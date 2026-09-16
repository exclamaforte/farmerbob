//! Cross-examination: reading a suite-by-implementation matrix.
//!
//! When N agents attack one task they produce N implementations and N test
//! suites. Running every suite against every implementation is N² cheap local
//! executions: no tokens, no model opinions. What comes back is a matrix of
//! [`Cell`]s, and the matrix carries a consensus signal that needs no judge.
//!
//! The signal lives in the *distribution* of a row or a column:
//!
//! - a suite that fails on 1 of 10 implementations, and passes on the other 9,
//!   is evidence of a defect in that one implementation;
//! - a suite that fails on 9 of 10 is evidence that the suite itself is wrong,
//!   or that it encodes an assumption the task never stated;
//! - a suite that passes on 10 of 10 discriminates nothing and is worth
//!   deleting.
//!
//! Both readings explain any single failure in isolation, so this module is
//! conservative by construction: it compares a cell against the *rest of its
//! row*, excludes measurements that do not exist ([`Cell::Error`]) or that are
//! not evidence (a suite run against the implementation it shipped with), and
//! refuses to conclude at all when the evidence is too thin or too evenly
//! split.
//!
//! Every function here is pure: no I/O, no clocks, no allocation beyond the
//! returned values.

use std::collections::BTreeSet;

/// Smallest number of implementations for which [`read`] will conclude anything.
///
/// A caller may raise this through [`read`]'s `min_impls`; it cannot lower it
/// below this floor. One or two implementations cannot separate "this
/// implementation is broken" from "this suite is wrong" — with two columns
/// every row is a coin flip — so [`read`] reports [`Reading::Inconclusive`]
/// for every failure instead of picking a side.
pub const MIN_CONCLUSIVE_IMPLS: usize = 3;

/// Outcome of running one suite against one implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    /// The suite passed against this implementation.
    Pass,
    /// The suite ran and failed: a real measurement of disagreement.
    Fail,
    /// No measurement: the suite did not build, did not run, or timed out.
    ///
    /// Excluded from every denominator. A suite that failed to compile against
    /// an implementation is a missing data point, not a defect.
    Error,
}

/// A square-ish matrix: suites (rows) by implementations (columns).
/// Suites and implementations are named; they need not be the same set.
///
/// Built through [`Matrix::new`], which enforces that the flat `cells` vector
/// matches the declared axes and that no axis repeats a name. Every
/// [`Matrix`] therefore satisfies `cells.len() == suites.len() * impls.len()`,
/// which is what lets the readers below index without checks that can fail.
#[derive(Debug, Clone, PartialEq)]
pub struct Matrix {
    suites: Vec<String>,
    impls: Vec<String>,
    cells: Vec<Cell>,
}

impl Matrix {
    /// `cells` is row-major: suite 0 against every impl, then suite 1, ...
    /// Errors if the length does not equal suites.len() * impls.len(), or if either
    /// axis contains a duplicate name.
    pub fn new(suites: Vec<String>, impls: Vec<String>, cells: Vec<Cell>) -> Result<Self, String> {
        let expected = suites
            .len()
            .checked_mul(impls.len())
            .ok_or_else(|| "matrix dimensions overflow".to_string())?;
        if cells.len() != expected {
            return Err(format!(
                "expected {expected} cells for {} suites x {} implementations, got {}",
                suites.len(),
                impls.len(),
                cells.len()
            ));
        }
        if let Some(dup) = first_duplicate(&suites) {
            return Err(format!("duplicate suite name: {dup}"));
        }
        if let Some(dup) = first_duplicate(&impls) {
            return Err(format!("duplicate implementation name: {dup}"));
        }
        Ok(Self {
            suites,
            impls,
            cells,
        })
    }

    /// The outcome of one named suite against one named implementation, or
    /// `None` when either name is not on its axis.
    pub fn get(&self, suite: &str, imp: &str) -> Option<Cell> {
        let row = self.suites.iter().position(|s| s == suite)?;
        let col = self.impls.iter().position(|i| i == imp)?;
        self.cell_at(row, col)
    }

    /// Suite names, in row order.
    pub fn suites(&self) -> &[String] {
        &self.suites
    }

    /// Implementation names, in column order.
    pub fn impls(&self) -> &[String] {
        &self.impls
    }

    /// The cells of one row, or an empty slice if the matrix is degenerate.
    fn row(&self, suite_index: usize) -> &[Cell] {
        let width = self.impls.len();
        let start = suite_index.saturating_mul(width);
        self.cells
            .get(start..start.saturating_add(width))
            .unwrap_or(&[])
    }

    /// One cell by position, or `None` when either index is off the matrix.
    fn cell_at(&self, suite_index: usize, impl_index: usize) -> Option<Cell> {
        self.row(suite_index).get(impl_index).copied()
    }
}

/// What the matrix says about one (suite, implementation) failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading {
    /// This suite fails here and passes nearly everywhere else.
    LikelyDefect {
        suite: String,
        imp: String,
        passed_elsewhere: usize,
        total_elsewhere: usize,
    },
    /// This suite fails nearly everywhere.
    LikelySuiteFault {
        suite: String,
        failed: usize,
        total: usize,
    },
    /// Too few implementations, or a split too even, to distinguish the two.
    Inconclusive {
        suite: String,
        imp: String,
        reason: String,
    },
}

impl Reading {
    /// The suite this reading is about.
    pub fn suite(&self) -> &str {
        match self {
            Self::LikelyDefect { suite, .. }
            | Self::LikelySuiteFault { suite, .. }
            | Self::Inconclusive { suite, .. } => suite,
        }
    }

    /// The implementation this reading is about, or `None` for a whole-suite
    /// reading that is not about any single implementation.
    pub fn imp(&self) -> Option<&str> {
        match self {
            Self::LikelyDefect { imp, .. } | Self::Inconclusive { imp, .. } => Some(imp),
            Self::LikelySuiteFault { .. } => None,
        }
    }
}

/// Read every failing cell. A cell is only reported once, under its strongest reading.
///
/// `min_impls` is the smallest number of implementations for which any conclusion is
/// allowed; below it every failure is `Inconclusive`. A caller passing 1 or 2 is asking
/// for noise, and this function must not oblige.
///
/// A failing cell is read against the rest of its row: the other implementations
/// that (a) produced a measurement — [`Cell::Error`] is excluded — and (b) are
/// not the implementation this suite shipped with. Passes on a strict majority
/// of those make it [`Reading::LikelyDefect`]; failures on a strict majority
/// make it [`Reading::LikelySuiteFault`]; an exact tie, no comparable
/// measurement at all, or fewer than `min_impls` implementations in the matrix
/// make it [`Reading::Inconclusive`].
///
/// [`Reading::LikelySuiteFault`] is a statement about a suite, so it is emitted
/// once per suite no matter how many of its cells fail. Readings are ordered by
/// suite (row order), and within a suite defects first, then the suite fault,
/// then inconclusive cells, each in column order.
pub fn read(m: &Matrix, min_impls: usize) -> Vec<Reading> {
    let floor = min_impls.max(MIN_CONCLUSIVE_IMPLS);
    let conclusive_allowed = m.impls.len() >= floor;

    let mut readings = Vec::new();
    for (suite_index, suite) in m.suites.iter().enumerate() {
        let mut defects: Vec<Reading> = Vec::new();
        let mut inconclusive: Vec<Reading> = Vec::new();
        let mut suite_fault: Option<Reading> = None;

        for (impl_index, imp) in m.impls.iter().enumerate() {
            if m.cell_at(suite_index, impl_index) != Some(Cell::Fail) {
                continue;
            }
            match read_cell(
                m,
                suite,
                imp,
                suite_index,
                impl_index,
                floor,
                conclusive_allowed,
            ) {
                r @ Reading::LikelyDefect { .. } => defects.push(r),
                r @ Reading::LikelySuiteFault { .. } => {
                    if suite_fault.is_none() {
                        suite_fault = Some(r);
                    }
                }
                r @ Reading::Inconclusive { .. } => inconclusive.push(r),
            }
        }

        readings.extend(defects);
        readings.extend(suite_fault);
        readings.extend(inconclusive);
    }
    readings
}

/// Read one failing cell against the rest of its row.
///
/// `floor` is the effective minimum number of implementations (the caller's
/// `min_impls` raised to [`MIN_CONCLUSIVE_IMPLS`]); `conclusive_allowed` is
/// whether the matrix has that many.
fn read_cell(
    m: &Matrix,
    suite: &str,
    imp: &str,
    suite_index: usize,
    impl_index: usize,
    floor: usize,
    conclusive_allowed: bool,
) -> Reading {
    let inconclusive = |reason: String| Reading::Inconclusive {
        suite: suite.to_string(),
        imp: imp.to_string(),
        reason,
    };

    if !conclusive_allowed {
        return inconclusive(format!(
            "only {} of at least {} implementations present; too few to separate a defect from a bad suite",
            m.impls.len(),
            floor
        ));
    }

    let mut passed = 0usize;
    let mut failed = 0usize;
    for (other_index, other) in m.impls.iter().enumerate() {
        if other_index == impl_index {
            continue;
        }
        // A suite run against the implementation it shipped with is not
        // evidence about anyone: exclude it from both counts.
        if other == suite {
            continue;
        }
        match m.cell_at(suite_index, other_index) {
            Some(Cell::Pass) => passed += 1,
            Some(Cell::Fail) => failed += 1,
            Some(Cell::Error) | None => {}
        }
    }
    let total = passed + failed;

    if total == 0 {
        return inconclusive(
            "no comparable implementation: every other measurement errored or is this suite's own implementation"
                .to_string(),
        );
    }

    // Strict majority, decided without floating point: 2 * n > total.
    if 2 * passed > total {
        return Reading::LikelyDefect {
            suite: suite.to_string(),
            imp: imp.to_string(),
            passed_elsewhere: passed,
            total_elsewhere: total,
        };
    }
    if 2 * failed > total {
        return Reading::LikelySuiteFault {
            suite: suite.to_string(),
            failed,
            total,
        };
    }
    inconclusive(format!(
        "even split among {total} comparable implementations: {passed} passed and {failed} failed"
    ))
}

/// Fraction of cells that are `Pass`, over cells that are not `Error`.
/// `None` when every cell errored: no denominator, and 0.0 would be a lie.
///
/// A matrix with no cells has no denominator either, and reports `None` for the
/// same reason.
pub fn agreement(m: &Matrix) -> Option<f64> {
    let mut measured = 0usize;
    let mut passed = 0usize;
    for cell in &m.cells {
        match cell {
            Cell::Pass => {
                measured += 1;
                passed += 1;
            }
            Cell::Fail => measured += 1,
            Cell::Error => {}
        }
    }
    if measured == 0 {
        return None;
    }
    Some(passed as f64 / measured as f64)
}

/// Suites whose row is identical across every implementation — they discriminate nothing.
/// These are the tests worth deleting, and the ones worth telling an author about.
///
/// Uniformity is judged on the row as recorded: an all-`Pass` row and an
/// all-`Fail` row are equally uninformative, and so is a row of nothing but
/// [`Cell::Error`], which measured nothing at all. Returned in suite order.
pub fn non_discriminating(m: &Matrix) -> Vec<String> {
    m.suites
        .iter()
        .enumerate()
        .filter(|(index, _)| row_is_uniform(m, *index))
        .map(|(_, suite)| suite.clone())
        .collect()
}

/// Whether every cell in a row is the same outcome.
///
/// A row of fewer than two cells cannot disagree with itself, so it is uniform:
/// whether one implementation passed or failed tells us nothing about any other.
fn row_is_uniform(m: &Matrix, suite_index: usize) -> bool {
    let row = m.row(suite_index);
    match row.first() {
        None => true,
        Some(first) => row.iter().all(|cell| cell == first),
    }
}

/// The first name that appears twice in `names`, if any.
fn first_duplicate(names: &[String]) -> Option<String> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for name in names {
        if !seen.insert(name.as_str()) {
            return Some(name.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a matrix from one row-string per suite: `P` pass, `F` fail, `E` error.
    fn matrix(suites: &[&str], impls: &[&str], rows: &[&str]) -> Matrix {
        let mut cells = Vec::new();
        for row in rows {
            assert_eq!(row.len(), impls.len(), "row width must match impls");
            for ch in row.chars() {
                cells.push(match ch {
                    'P' => Cell::Pass,
                    'F' => Cell::Fail,
                    'E' => Cell::Error,
                    other => panic!("bad cell char: {other}"),
                });
            }
        }
        Matrix::new(
            suites.iter().map(|s| s.to_string()).collect(),
            impls.iter().map(|s| s.to_string()).collect(),
            cells,
        )
        .expect("well-formed matrix")
    }

    /// `n` implementations named `i0` .. `i{n-1}`.
    fn impls(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("i{i}")).collect()
    }

    /// A matrix with one suite per row, suites named `s0`, `s1`, ... and
    /// implementations named `i0`, `i1`, ...
    fn square(rows: &[&str]) -> Matrix {
        let width = rows.first().map_or(0, |r| r.len());
        let suites: Vec<String> = (0..rows.len()).map(|i| format!("s{i}")).collect();
        let names: Vec<String> = impls(width);
        let suite_refs: Vec<&str> = suites.iter().map(String::as_str).collect();
        let impl_refs: Vec<&str> = names.iter().map(String::as_str).collect();
        matrix(&suite_refs, &impl_refs, rows)
    }

    fn is_defect(r: &Reading) -> bool {
        matches!(r, Reading::LikelyDefect { .. })
    }

    fn is_suite_fault(r: &Reading) -> bool {
        matches!(r, Reading::LikelySuiteFault { .. })
    }

    fn is_inconclusive(r: &Reading) -> bool {
        matches!(r, Reading::Inconclusive { .. })
    }

    #[test]
    fn one_of_ten_failure_reads_as_likely_defect() {
        // Ten implementations, one suite, a single failure: the suite is fine,
        // the implementation is not.
        let m = square(&["PPPPFPPPPP"]);
        let readings = read(&m, 3);

        assert_eq!(
            readings.len(),
            1,
            "one failing cell, one reading: {readings:?}"
        );
        match &readings[0] {
            Reading::LikelyDefect {
                suite,
                imp,
                passed_elsewhere,
                total_elsewhere,
            } => {
                assert_eq!(suite, "s0");
                assert_eq!(imp, "i4");
                assert_eq!(*total_elsewhere, 9, "all nine other impls were measured");
                assert_eq!(*passed_elsewhere, 9, "all nine passed");
            }
            other => panic!("expected a defect, got {other:?}"),
        }
    }

    #[test]
    fn nine_of_ten_failure_reads_as_suite_fault_once() {
        // Nine of ten fail. The suite is the common factor, and the reading is
        // about the suite, not about nine separate implementations.
        let m = square(&["PFFFFFFFFF"]);
        let readings = read(&m, 3);

        assert_eq!(
            readings.len(),
            1,
            "reported once, not per cell: {readings:?}"
        );
        match &readings[0] {
            Reading::LikelySuiteFault {
                suite,
                failed,
                total,
            } => {
                assert_eq!(suite, "s0");
                assert_eq!(*total, 9, "nine comparable other impls");
                assert_eq!(*failed, 8, "the other eight failures, errors excluded");
            }
            other => panic!("expected a suite fault, got {other:?}"),
        }
    }

    #[test]
    fn suite_fault_is_reported_once_per_suite() {
        // Two suites, both failing nearly everywhere: two readings, one each.
        let m = square(&["PFFFFFFFFF", "PFFFFFFFFF"]);
        let readings = read(&m, 3);

        let faults: Vec<&String> = readings
            .iter()
            .filter_map(|r| match r {
                Reading::LikelySuiteFault { suite, .. } => Some(suite),
                _ => None,
            })
            .collect();
        assert_eq!(faults, vec!["s0", "s1"]);
        assert_eq!(
            readings.len(),
            2,
            "one per suite, not per cell: {readings:?}"
        );
    }

    #[test]
    fn even_split_is_inconclusive() {
        // Three implementations, two failures, one pass: each failing cell sees
        // one pass and one failure. Neither reading wins.
        let m = square(&["FFP"]);
        let readings = read(&m, 3);

        assert_eq!(readings.len(), 2, "one reading per failing cell");
        assert!(readings.iter().all(is_inconclusive), "{readings:?}");
        for r in &readings {
            match r {
                Reading::Inconclusive { reason, .. } => {
                    assert!(!reason.is_empty(), "an inconclusive reading must say why")
                }
                other => panic!("expected inconclusive, got {other:?}"),
            }
        }
    }

    #[test]
    fn even_split_with_an_error_is_still_inconclusive() {
        // Ten implementations: five fail, four pass, one errored. Each failing
        // cell sees four passes and four failures among the measured.
        let m = square(&["FFFFFPPPPE"]);
        let readings = read(&m, 3);
        assert!(readings.iter().all(is_inconclusive), "{readings:?}");
        assert_eq!(readings.len(), 5, "one reading per failing cell");
    }

    #[test]
    fn min_impls_suppresses_conclusions_on_a_two_column_matrix() {
        // Two implementations. One passes, one fails: indistinguishable from a
        // bad suite. No `min_impls` may buy a conclusion here.
        let m = square(&["FP"]);
        for min_impls in [0usize, 1, 2, 3, 4, 9] {
            let readings = read(&m, min_impls);
            assert_eq!(readings.len(), 1, "the failure is still reported");
            assert!(
                readings.iter().all(is_inconclusive),
                "min_impls={min_impls} must not conclude on two columns: {readings:?}"
            );
        }
    }

    #[test]
    fn min_impls_floor_does_not_suppress_a_wide_matrix() {
        // The floor refuses thin evidence; it does not refuse all evidence.
        let m = square(&["FPP"]);
        let readings = read(&m, 3);
        assert!(
            readings.iter().all(is_defect),
            "three impls, two passes: a defect {readings:?}"
        );
    }

    #[test]
    fn min_impls_above_the_column_count_suppresses() {
        let m = square(&["PPPPFPPPPP"]);
        let readings = read(&m, 11);
        assert!(
            readings.iter().all(is_inconclusive),
            "asking for 11 impls when 10 are present: {readings:?}"
        );
    }

    #[test]
    fn error_cells_are_not_counted_as_failures() {
        // One failure, four passes, five errors. The errors are missing
        // measurements, so the denominator is four, not nine.
        let m = square(&["FEEEEEPPPP"]);
        let readings = read(&m, 3);

        assert_eq!(
            readings.len(),
            1,
            "errors produce no readings: {readings:?}"
        );
        match &readings[0] {
            Reading::LikelyDefect {
                passed_elsewhere,
                total_elsewhere,
                ..
            } => {
                assert_eq!(*total_elsewhere, 4, "only measured cells count");
                assert_eq!(*passed_elsewhere, 4, "all measured cells passed");
            }
            other => panic!("expected a defect, got {other:?}"),
        }
    }

    #[test]
    fn error_cells_never_produce_readings() {
        let m = square(&["EEEE", "EEEE"]);
        assert!(read(&m, 3).is_empty(), "an unmeasured suite says nothing");
        assert_eq!(agreement(&m), None);
    }

    #[test]
    fn a_suite_that_only_ever_fails_on_errored_neighbours_is_inconclusive() {
        // The failing cell's whole row apart from itself errored.
        let m = matrix(&["x"], &["x", "b", "c"], &["FEE"]);
        let readings = read(&m, 3);

        assert_eq!(readings.len(), 1, "{readings:?}");
        match &readings[0] {
            Reading::Inconclusive { reason, .. } => assert!(
                reason.to_lowercase().contains("error"),
                "the reason must say the comparison was empty: {reason}"
            ),
            other => panic!("expected inconclusive, got {other:?}"),
        }
    }

    #[test]
    fn agreement_is_none_on_an_all_error_matrix() {
        let m = matrix(&["s0", "s1"], &["i0", "i1", "i2"], &["EEE", "EEE"]);
        assert_eq!(
            agreement(&m),
            None,
            "no denominator, and 0.0 would be a lie"
        );
    }

    #[test]
    fn agreement_is_none_on_an_empty_matrix() {
        let m = Matrix::new(Vec::new(), Vec::new(), Vec::new()).expect("0 x 0 is well formed");
        assert_eq!(agreement(&m), None);
    }

    #[test]
    fn agreement_counts_passes_over_measured_cells() {
        let m = matrix(&["s0"], &["i0", "i1", "i2", "i3"], &["PFPE"]);
        let a = agreement(&m).expect("measured cells exist");
        assert!((a - 2.0 / 3.0).abs() < 1e-9, "two of three measured: {a}");
    }

    #[test]
    fn all_pass_and_all_fail_rows_are_non_discriminating() {
        // Both carry no information: everyone agrees, whether on pass or fail.
        let m = matrix(
            &["all_pass", "all_fail", "mixed"],
            &["i0", "i1", "i2"],
            &["PPP", "FFF", "PFP"],
        );
        assert_eq!(non_discriminating(&m), vec!["all_pass", "all_fail"]);
    }

    #[test]
    fn a_row_that_differs_anywhere_discriminates() {
        let m = matrix(&["differs"], &["i0", "i1", "i2"], &["PFE"]);
        assert!(non_discriminating(&m).is_empty());
    }

    #[test]
    fn self_pair_does_not_inflate_passed_elsewhere() {
        // Suite "a" shipped with implementation "a". It fails on "i1". Its own
        // pass on itself is not evidence, so the comparison has eight members,
        // not nine.
        let m = matrix(
            &["a"],
            &["a", "i1", "i2", "i3", "i4", "i5", "i6", "i7", "i8", "i9"],
            &["PFPPPPPPPP"],
        );
        let readings = read(&m, 3);

        assert_eq!(readings.len(), 1, "{readings:?}");
        match &readings[0] {
            Reading::LikelyDefect {
                imp,
                passed_elsewhere,
                total_elsewhere,
                ..
            } => {
                assert_eq!(imp, "i1");
                assert_eq!(*total_elsewhere, 8, "the self-pair is not comparable");
                assert_eq!(*passed_elsewhere, 8);
            }
            other => panic!("expected a defect, got {other:?}"),
        }
    }

    #[test]
    fn self_pair_does_not_inflate_failed_counts() {
        // Suite "a" is also implementation "a". Failures on a, b and c, a pass
        // on d. Reading cell (a, b) must exclude the self-pair a: one pass and
        // one failure remain, which is a tie, not a suite fault.
        let m = matrix(&["a"], &["a", "b", "c", "d"], &["FFFP"]);
        let readings = read(&m, 3);

        let for_b: Vec<&Reading> = readings.iter().filter(|r| r.imp() == Some("b")).collect();
        assert_eq!(for_b.len(), 1, "one reading for cell (a, b): {readings:?}");
        assert!(
            is_inconclusive(for_b[0]),
            "excluding the self-pair leaves a tie: {readings:?}"
        );
        assert_eq!(
            readings.iter().filter(|r| is_suite_fault(r)).count(),
            1,
            "the suite fault, if any, is reported once: {readings:?}"
        );
    }

    #[test]
    fn a_failure_on_the_suite_own_implementation_is_still_read() {
        // Self-pairs are in the matrix; only their contribution to *other*
        // cells' counts is withheld.
        let m = matrix(&["a"], &["a", "i1", "i2"], &["FPP"]);
        let readings = read(&m, 3);
        assert_eq!(readings.len(), 1, "the failing cell is read: {readings:?}");
        assert_eq!(readings[0].imp(), Some("a"));
        assert!(is_defect(&readings[0]), "both others passed: {readings:?}");
    }

    #[test]
    fn nothing_fails_means_nothing_to_read() {
        let m = square(&["PPP", "PPP"]);
        assert!(read(&m, 3).is_empty());
    }

    #[test]
    fn reads_a_non_square_matrix() {
        // Three suites, four implementations: the axes need not match.
        let m = matrix(
            &["s0", "s1", "s2"],
            &["i0", "i1", "i2", "i3"],
            &["PPPF", "PFFF", "PPPP"],
        );
        let readings = read(&m, 3);

        let for_s0: Vec<&Reading> = readings.iter().filter(|r| r.suite() == "s0").collect();
        assert_eq!(for_s0.len(), 1, "{readings:?}");
        match for_s0[0] {
            Reading::LikelyDefect {
                imp,
                passed_elsewhere,
                total_elsewhere,
                ..
            } => {
                assert_eq!(imp, "i3");
                assert_eq!((*passed_elsewhere, *total_elsewhere), (3, 3));
            }
            other => panic!("expected a defect for s0/i3, got {other:?}"),
        }
        // s1 fails on three of four: a suite fault, once.
        assert_eq!(
            readings.iter().filter(|r| is_suite_fault(r)).count(),
            1,
            "{readings:?}"
        );
        assert_eq!(agreement(&m), Some(8.0 / 12.0));
    }

    #[test]
    fn new_rejects_a_cell_count_that_does_not_match_the_axes() {
        let suites = vec!["s0".to_string()];
        let impls = impls(3);
        let too_few = Matrix::new(suites.clone(), impls.clone(), vec![Cell::Pass]);
        assert!(too_few.is_err());
        let too_many = Matrix::new(suites, impls, vec![Cell::Pass; 4]);
        assert!(too_many.is_err());
    }

    #[test]
    fn new_rejects_duplicate_names_on_either_axis() {
        let dup_suites = Matrix::new(
            vec!["s".to_string(), "s".to_string()],
            impls(2),
            vec![Cell::Pass; 4],
        );
        assert!(dup_suites.is_err(), "suites must be distinct");

        let dup_impls = Matrix::new(
            vec!["s0".to_string()],
            vec!["i".to_string(), "i".to_string()],
            vec![Cell::Pass; 2],
        );
        assert!(dup_impls.is_err(), "implementations must be distinct");
    }

    #[test]
    fn get_returns_the_cell_at_a_named_crossing() {
        let m = matrix(&["s0", "s1"], &["i0", "i1"], &["PF", "EP"]);
        assert_eq!(m.get("s0", "i0"), Some(Cell::Pass));
        assert_eq!(m.get("s0", "i1"), Some(Cell::Fail));
        assert_eq!(m.get("s1", "i0"), Some(Cell::Error));
        assert_eq!(m.get("s1", "i1"), Some(Cell::Pass));
        assert_eq!(m.get("s0", "nope"), None);
        assert_eq!(m.get("nope", "i0"), None);
        assert_eq!(m.suites(), ["s0", "s1"]);
        assert_eq!(m.impls(), ["i0", "i1"]);
    }

    #[test]
    fn read_is_deterministic() {
        let m = matrix(&["s0", "s1"], &["i0", "i1", "i2", "i3"], &["PFFP", "FPPP"]);
        let first = read(&m, 3);
        for _ in 0..16 {
            assert_eq!(read(&m, 3), first, "same matrix, same readings");
        }
    }

    #[test]
    fn every_reading_names_a_suite_on_the_matrix() {
        // No reading may invent a name, and no reading may be about a cell
        // that did not fail.
        let m = matrix(
            &["s0", "s1", "s2"],
            &["s0", "i1", "i2", "i3"],
            &["FFPF", "EEEE", "PFFP"],
        );
        let readings = read(&m, 3);
        for r in &readings {
            assert!(m.suites().iter().any(|s| s == r.suite()), "{r:?}");
            if let Some(imp) = r.imp() {
                assert!(m.impls().iter().any(|i| i == imp), "{r:?}");
                assert_eq!(
                    m.get(r.suite(), imp),
                    Some(Cell::Fail),
                    "only failing cells are read: {r:?}"
                );
            }
        }
    }
}
