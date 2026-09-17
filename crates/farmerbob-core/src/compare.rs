//! The comparison view a planning agent reads before picking a winner.
//!
//! This is the model behind the side-by-side table of candidate runs for one
//! task: one [`Row`] per candidate, plus the pure functions an orchestrator
//! uses to order, discriminate, and stringify those rows. It performs no I/O
//! and does no terminal formatting; it produces the data a view renders.
//!
//! The central honesty constraint is that an absent measurement must survive
//! as `None` all the way to the rendered cell. A `None` is never a zero, a
//! zero is never a `None`, and an unmeasured cell is never a losing cell:
//! [`compare_on`] and [`rank`] treat absence as `Incomparable` / last, never
//! as worse. The view can also declare candidates indistinguishable, because
//! most fields tie and a ranking that always yields an order would manufacture
//! a winner from noise.

use std::cmp::Ordering;

/// One candidate's row in the comparison table.
///
/// Every metric is optional. `None` means the quantity was not measured for
/// this candidate, and must be rendered as `n/a`, never as `0`.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The arm (candidate strategy) that produced this run.
    pub arm: String,
    /// The verdict string, e.g. "pass", "fail", "void".
    pub verdict: String,
    /// Lines of code, when measured.
    pub lines: Option<u32>,
    /// Tests passing, when measured.
    pub tests: Option<u32>,
    /// Clippy warnings introduced, as a DELTA against the base. May be negative.
    pub clippy_delta: Option<i32>,
    /// Conformance score in `[0, 1]`.
    pub conformance: Option<f64>,
    /// Fraction of rivals' discriminating suites this implementation survives,
    /// in `[0, 1]`.
    pub survival: Option<f64>,
    /// Measured USD. `Some(0.0)` is a real zero; `None` is unmeasured.
    pub usd: Option<f64>,
    /// Measured wall-clock seconds.
    pub secs: Option<u64>,
}

/// Which column an ordering is over. Exactly these and no others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    /// `Row::conformance`
    Conformance,
    /// `Row::survival`
    Survival,
    /// `Row::clippy_delta`
    ClippyDelta,
    /// `Row::tests`
    Tests,
    /// `Row::lines`
    Lines,
    /// `Row::usd`
    Usd,
    /// `Row::secs`
    Secs,
}

impl Column {
    /// True when a LOWER value is better.
    ///
    /// This holds for exactly [`Column::ClippyDelta`], [`Column::Lines`],
    /// [`Column::Usd`], and [`Column::Secs`]; it is false for the rest.
    pub fn lower_is_better(self) -> bool {
        matches!(
            self,
            Column::ClippyDelta | Column::Lines | Column::Usd | Column::Secs
        )
    }
}

/// How two rows compare on one column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmp {
    /// The first row is strictly better on this column.
    Better,
    /// The first row is strictly worse on this column.
    Worse,
    /// The two rows are tied (within `epsilon` for float columns).
    Tied,
    /// At least one value is absent or NaN, so no ordering is defined.
    Incomparable,
}

/// Compare two rows on one column.
///
/// Returns [`Cmp::Incomparable`] when EITHER row's value for the column is
/// `None`, or when either value is a NaN. This is checked before any numeric
/// comparison, so an absent or NaN value can never read as better or worse.
/// For float columns a difference at or below `epsilon` is [`Cmp::Tied`];
/// integer columns compare exactly.
pub fn compare_on(a: &Row, b: &Row, col: Column, epsilon: f64) -> Cmp {
    match (present(a, col), present(b, col)) {
        (None, _) | (_, None) => Cmp::Incomparable,
        (Some((x, is_float)), Some((y, _))) => {
            let eps = if is_float { epsilon } else { 0.0 };
            if (x - y).abs() <= eps {
                Cmp::Tied
            } else if x > y {
                // Higher "goodness" (already sign-flipped for lower-is-better)
                // means strictly better.
                Cmp::Better
            } else {
                Cmp::Worse
            }
        }
    }
}

/// Rows ordered best-first on one column.
///
/// Rows with an absent (or NaN) value are placed LAST regardless of direction,
/// and present rows keep their input order relative to one another (the sort
/// is stable). Among present values, a difference at or below `epsilon` for
/// float columns counts as a tie and also preserves input order.
pub fn rank(rows: &[Row], col: Column, epsilon: f64) -> Vec<&Row> {
    let mut v: Vec<&Row> = rows.iter().collect();
    v.sort_by(|a, b| rank_cmp(a, b, col, epsilon));
    v
}

/// Columns on which the field is not entirely tied or absent.
///
/// A column is returned only if at least two rows have a measured value AND
/// those present values are not all tied. Returned in [`Column`] declaration
/// order. An empty `rows` yields an empty vector.
pub fn discriminating_columns(rows: &[Row], epsilon: f64) -> Vec<Column> {
    use Column::*;
    [Conformance, Survival, ClippyDelta, Tests, Lines, Usd, Secs]
        .into_iter()
        .filter(|&col| is_discriminating(rows, col, epsilon))
        .collect()
}

/// A one-line-per-row table of stringified cells.
///
/// Returns `(header, body)`. Every `None` value renders as the literal `"n/a"`
/// and never as `"0"`; a negative `clippy_delta` keeps its sign. An empty
/// `rows` yields an empty body (and an empty `header` when `cols` is empty),
/// not an error.
pub fn render(rows: &[Row], cols: &[Column]) -> (Vec<String>, Vec<Vec<String>>) {
    let header: Vec<String> = cols.iter().map(|&c| column_name(&c).to_string()).collect();
    let body: Vec<Vec<String>> = rows
        .iter()
        .map(|r| cols.iter().map(|&c| cell(r, c)).collect())
        .collect();
    (header, body)
}

/// The "goodness" of a present value: `Some((goodness, is_float))` where a
/// higher `goodness` means a better row, or `None` when the value is absent or
/// NaN. For lower-is-better columns the value is sign-flipped so the same
/// "higher is better" rule applies everywhere.
fn present(row: &Row, col: Column) -> Option<(f64, bool)> {
    let lib = col.lower_is_better();
    match col {
        Column::Conformance => map_float(row.conformance, lib),
        Column::Survival => map_float(row.survival, lib),
        Column::Usd => map_float(row.usd, lib),
        Column::ClippyDelta => map_int(row.clippy_delta.map(|v| v as f64), lib),
        Column::Tests => map_int(row.tests.map(|v| v as f64), lib),
        Column::Lines => map_int(row.lines.map(|v| v as f64), lib),
        Column::Secs => map_int(row.secs.map(|v| v as f64), lib),
    }
}

fn map_float(v: Option<f64>, lower_is_better: bool) -> Option<(f64, bool)> {
    match v {
        Some(x) if x.is_nan() => None,
        Some(x) => Some((if lower_is_better { -x } else { x }, true)),
        None => None,
    }
}

fn map_int(v: Option<f64>, lower_is_better: bool) -> Option<(f64, bool)> {
    v.map(|x| (if lower_is_better { -x } else { x }, false))
}

fn rank_cmp(a: &Row, b: &Row, col: Column, epsilon: f64) -> Ordering {
    match (present(a, col), present(b, col)) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some((x, is_float)), Some((y, _))) => {
            let eps = if is_float { epsilon } else { 0.0 };
            if (x - y).abs() <= eps {
                Ordering::Equal
            } else if x > y {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        }
    }
}

fn is_discriminating(rows: &[Row], col: Column, epsilon: f64) -> bool {
    let present_rows: Vec<&Row> = rows.iter().filter(|r| present(r, col).is_some()).collect();
    if present_rows.len() < 2 {
        return false;
    }
    for i in 0..present_rows.len() {
        for j in (i + 1)..present_rows.len() {
            match compare_on(present_rows[i], present_rows[j], col, epsilon) {
                Cmp::Tied | Cmp::Incomparable => {}
                Cmp::Better | Cmp::Worse => return true,
            }
        }
    }
    false
}

fn column_name(col: &Column) -> &'static str {
    match col {
        Column::Conformance => "Conformance",
        Column::Survival => "Survival",
        Column::ClippyDelta => "ClippyDelta",
        Column::Tests => "Tests",
        Column::Lines => "Lines",
        Column::Usd => "Usd",
        Column::Secs => "Secs",
    }
}

fn cell(row: &Row, col: Column) -> String {
    match col {
        Column::Conformance => fmt_float(row.conformance),
        Column::Survival => fmt_float(row.survival),
        Column::Usd => fmt_float(row.usd),
        Column::ClippyDelta => match row.clippy_delta {
            None => "n/a".to_string(),
            Some(v) => v.to_string(),
        },
        Column::Tests => match row.tests {
            None => "n/a".to_string(),
            Some(v) => v.to_string(),
        },
        Column::Lines => match row.lines {
            None => "n/a".to_string(),
            Some(v) => v.to_string(),
        },
        Column::Secs => match row.secs {
            None => "n/a".to_string(),
            Some(v) => v.to_string(),
        },
    }
}

fn fmt_float(v: Option<f64>) -> String {
    match v {
        None => "n/a".to_string(),
        // A NaN is treated as absent rather than ordered, but it should never
        // reach here from `present`; render defensively anyway.
        Some(x) if x.is_nan() => "n/a".to_string(),
        Some(x) => x.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(arm: &str) -> Row {
        Row {
            arm: arm.to_string(),
            verdict: "pass".to_string(),
            lines: None,
            tests: None,
            clippy_delta: None,
            conformance: None,
            survival: None,
            usd: None,
            secs: None,
        }
    }

    #[test]
    fn absent_is_incomparable_not_worse() {
        let a = row("a");
        let mut b = row("b");
        b.tests = Some(10);
        let c = compare_on(&a, &b, Column::Tests, 1e-9);
        assert_eq!(c, Cmp::Incomparable);
        // And the reverse direction is also Incomparable, never a win for b.
        assert_eq!(compare_on(&b, &a, Column::Tests, 1e-9), Cmp::Incomparable);
    }

    #[test]
    fn absent_sorts_last_ascending() {
        // Tests: higher is better (ascending best-first => high first).
        let mut lo = row("lo");
        lo.tests = Some(1);
        let mut hi = row("hi");
        hi.tests = Some(9);
        let missing = row("missing");

        let mut rows = vec![missing.clone(), lo.clone(), hi.clone()];
        let ranked = rank(&rows, Column::Tests, 1e-9);
        let order: Vec<&str> = ranked.iter().map(|r| r.arm.as_str()).collect();
        assert_eq!(order, vec!["hi", "lo", "missing"]);

        // Reverse input order to confirm the missing row still lands last.
        rows = vec![hi.clone(), missing.clone(), lo.clone()];
        let ranked = rank(&rows, Column::Tests, 1e-9);
        let order: Vec<&str> = ranked.iter().map(|r| r.arm.as_str()).collect();
        assert_eq!(order, vec!["hi", "lo", "missing"]);
    }

    #[test]
    fn absent_sorts_last_descending() {
        // Lines: lower is better (descending best-first => low first).
        let mut lo = row("lo");
        lo.lines = Some(1);
        let mut hi = row("hi");
        hi.lines = Some(9);
        let missing = row("missing");

        let rows = vec![hi.clone(), missing.clone(), lo.clone()];
        let ranked = rank(&rows, Column::Lines, 1e-9);
        let order: Vec<&str> = ranked.iter().map(|r| r.arm.as_str()).collect();
        assert_eq!(order, vec!["lo", "hi", "missing"]);
    }

    #[test]
    fn nan_treated_as_absent() {
        let mut a = row("a");
        a.conformance = Some(f64::NAN);
        let mut b = row("b");
        b.conformance = Some(0.9);
        assert_eq!(
            compare_on(&a, &b, Column::Conformance, 1e-9),
            Cmp::Incomparable
        );
        assert_eq!(
            compare_on(&b, &a, Column::Conformance, 1e-9),
            Cmp::Incomparable
        );

        // And it sorts last, like any absent value.
        let arr = [a.clone(), b.clone()];
        let ranked = rank(&arr, Column::Conformance, 1e-9);
        assert_eq!(ranked[1].arm, "a");
    }

    #[test]
    fn one_present_value_not_discriminating() {
        let mut a = row("a");
        a.tests = Some(5);
        let b = row("b");
        let c = row("c");
        let cols = discriminating_columns(&[a, b, c], 1e-9);
        assert!(!cols.contains(&Column::Tests));
    }

    #[test]
    fn na_never_renders_as_zero() {
        let r = row("a");
        let (_, body) = render(&[r], &[Column::Tests, Column::Usd, Column::Lines]);
        assert_eq!(body[0], vec!["n/a", "n/a", "n/a"]);

        // A genuine zero is still a zero, and distinct from absent.
        let mut z = row("z");
        z.tests = Some(0);
        z.usd = Some(0.0);
        let (_, body) = render(&[z], &[Column::Tests, Column::Usd]);
        assert_eq!(body[0], vec!["0", "0"]);
    }

    #[test]
    fn negative_clippy_delta_keeps_sign() {
        let mut r = row("a");
        r.clippy_delta = Some(-3);
        let (_, body) = render(&[r], &[Column::ClippyDelta]);
        assert_eq!(body[0], vec!["-3"]);
    }

    #[test]
    fn ranking_is_stable_among_ties() {
        // All tied on a higher-is-better column: input order preserved.
        let mut a = row("a");
        a.tests = Some(5);
        let mut b = row("b");
        b.tests = Some(5);
        let mut c = row("c");
        c.tests = Some(5);
        let rows = vec![a.clone(), b.clone(), c.clone()];
        let ranked = rank(&rows, Column::Tests, 1e-9);
        let order: Vec<&str> = ranked.iter().map(|r| r.arm.as_str()).collect();
        assert_eq!(order, vec!["a", "b", "c"]);

        // Mixed: two present tied, one absent; present keep order, absent last.
        let mut d = row("d");
        d.tests = Some(5);
        let mut e = row("e");
        e.tests = Some(5);
        let miss = row("miss");
        let arr = [miss.clone(), d.clone(), e.clone()];
        let ranked = rank(&arr, Column::Tests, 1e-9);
        let order: Vec<&str> = ranked.iter().map(|r| r.arm.as_str()).collect();
        assert_eq!(order, vec!["d", "e", "miss"]);
    }

    #[test]
    fn empty_field_is_not_an_error() {
        let empty: Vec<Row> = vec![];
        let (header, body) = render(&empty, &[Column::Tests, Column::Usd]);
        assert!(header.is_empty() || header.len() == 2);
        assert!(body.is_empty());
        assert!(discriminating_columns(&empty, 1e-9).is_empty());
        assert!(rank(&empty, Column::Tests, 1e-9).is_empty());
    }

    #[test]
    fn epsilon_ties_floats() {
        let mut a = row("a");
        a.conformance = Some(0.900);
        let mut b = row("b");
        b.conformance = Some(0.9001);
        assert_eq!(compare_on(&a, &b, Column::Conformance, 1e-2), Cmp::Tied);
        assert_eq!(
            compare_on(&a, &b, Column::Conformance, 1e-9),
            Cmp::Worse // a slightly lower => worse on higher-is-better
        );
    }

    #[test]
    fn all_tied_column_not_discriminating() {
        let mut a = row("a");
        a.tests = Some(5);
        let mut b = row("b");
        b.tests = Some(5);
        let cols = discriminating_columns(&[a, b], 1e-9);
        assert!(!cols.contains(&Column::Tests));
    }

    #[test]
    fn lower_is_better_directions() {
        assert!(Column::ClippyDelta.lower_is_better());
        assert!(Column::Lines.lower_is_better());
        assert!(Column::Usd.lower_is_better());
        assert!(Column::Secs.lower_is_better());
        assert!(!Column::Conformance.lower_is_better());
        assert!(!Column::Survival.lower_is_better());
        assert!(!Column::Tests.lower_is_better());
    }

    #[test]
    fn discriminating_finds_real_difference() {
        let mut a = row("a");
        a.tests = Some(5);
        let mut b = row("b");
        b.tests = Some(8);
        let cols = discriminating_columns(&[a, b], 1e-9);
        assert!(cols.contains(&Column::Tests));
    }
}
