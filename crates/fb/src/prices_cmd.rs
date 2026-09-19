//! Price-change reporting for `fb-prices`.
//!
//! Unwired from the argument parser by design: a separate task adds the
//! subcommand. Until then the public functions are reachable but unused,
//! which trips `dead_code` in a binary crate, so the module suppresses it.
#![allow(dead_code)]

use farmerbob_core::price_update::Change;

/// Render the pending price changes.
///
/// One line per registered model whose outcome is not `Agrees`, in the order
/// `price_update::pending` returns them. A model that agrees is not printed.
pub fn table(pending: &[(String, Change)]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for (model, change) in pending {
        match change {
            Change::Agrees => {
                // Skip agreed entries.
            }
            Change::Differs {
                price_in,
                price_out,
            } => {
                lines.push(format!(
                    "{}: differs (in={}, out={})",
                    model, price_in, price_out
                ));
            }
            Change::Fills {
                price_in,
                price_out,
            } => {
                lines.push(format!(
                    "{}: fills (in={}, out={})",
                    model, price_in, price_out
                ));
            }
            Change::Unquoted => {
                // Never treat an absent catalogue price as free (no `0` figure).
                lines.push(format!("{}: unquoted — price not listed", model));
            }
        }
    }
    if lines.is_empty() {
        "nothing pending".to_string()
    } else {
        lines.join("\n")
    }
}

/// Render and write. Returns the exit code the caller should use.
pub fn run(pending: &[(String, Change)], out: &mut dyn std::io::Write) -> i32 {
    let output = table(pending);
    // Write the string; a trailing newline keeps the channel readable.
    let _ = out.write_all(output.as_bytes());
    let _ = out.write_all(b"\n");
    if pending.iter().any(|(_, c)| !matches!(c, Change::Agrees)) {
        1
    } else {
        0
    }
}

/// Whether any pending change would OVERWRITE an existing figure, as opposed
/// to filling a blank.
pub fn has_overwrite(pending: &[(String, Change)]) -> bool {
    pending
        .iter()
        .any(|(_, c)| matches!(c, Change::Differs { .. }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use farmerbob_core::price_update::Change;

    fn makes_differs(price_in: u64, price_out: u64) -> Change {
        Change::Differs {
            price_in,
            price_out,
        }
    }

    fn makes_fills(price_in: u64, price_out: u64) -> Change {
        Change::Fills {
            price_in,
            price_out,
        }
    }

    #[test]
    fn clause_1_empty_slice_exits_zero_and_non_empty_output() {
        let pending: Vec<(String, Change)> = vec![];
        let mut out = Vec::new();
        assert_eq!(run(&pending, &mut out), 0);
        let text = String::from_utf8(out).unwrap();
        assert!(!text.trim().is_empty(), "output must not be empty");
        assert!(
            text.to_lowercase().contains("nothing") || text.to_lowercase().contains("pending"),
            "output should say plainly nothing is pending: {text:?}"
        );
    }

    #[test]
    fn clause_2_differs_line_and_exit_one() {
        let pending = vec![("model-a".to_string(), makes_differs(100, 200))];
        let out = table(&pending);
        assert!(out.contains("model-a"));
        assert!(out.contains("100"));
        assert!(out.contains("200"));
        assert!(out.contains("differs"));

        let mut buf = Vec::new();
        assert_eq!(run(&pending, &mut buf), 1);
    }

    #[test]
    fn clause_3_fills_line_differs_from_differs() {
        let diff = vec![("m".to_string(), makes_differs(10, 20))];
        let fill = vec![("m".to_string(), makes_fills(10, 20))];
        let d_line = table(&diff);
        let f_line = table(&fill);
        assert_ne!(
            d_line, f_line,
            "Fills and Differs must render differently for same model and figures"
        );
        assert!(f_line.contains("fills"));
        assert!(d_line.contains("differs"));
        assert!(f_line.contains("10"));
        assert!(f_line.contains("20"));
    }

    #[test]
    fn clause_4_unquoted_renders_exit_one_no_zero_figure() {
        let pending = vec![("missing-model".to_string(), Change::Unquoted)];
        let out = table(&pending);
        assert!(
            !out.contains('0'),
            "unquoted line must not contain a zero price figure: {out:?}"
        );
        assert!(out.contains("missing-model"));

        let mut buf = Vec::new();
        assert_eq!(run(&pending, &mut buf), 1);
    }

    #[test]
    fn clause_5_agrees_not_printed() {
        let pending = vec![("agree-model".to_string(), Change::Agrees)];
        let out = table(&pending);
        assert!(!out.contains("agree-model"));
        assert!(out.contains("nothing pending") || !out.trim().is_empty());
        let mut buf = Vec::new();
        assert_eq!(run(&pending, &mut buf), 0);
    }

    #[test]
    fn clause_6_has_overwrite_true_for_differs_false_for_others() {
        // True when any Differs
        assert!(has_overwrite(&[("a".to_string(), makes_differs(1, 2))]));

        // False when every entry is Fills
        assert!(!has_overwrite(&[("a".to_string(), makes_fills(1, 2))]));

        // False when catalogue-missing (Unquoted)
        assert!(!has_overwrite(&[("a".to_string(), Change::Unquoted)]));

        // False when Agrees
        assert!(!has_overwrite(&[("a".to_string(), Change::Agrees)]));

        // False for mixed Fills + Unquoted
        assert!(!has_overwrite(&[
            ("a".to_string(), makes_fills(1, 2)),
            ("b".to_string(), Change::Unquoted),
        ]));

        // True for mixed with Differs
        assert!(has_overwrite(&[
            ("a".to_string(), makes_fills(1, 2)),
            ("b".to_string(), makes_differs(3, 4)),
        ]));
    }

    #[test]
    fn clause_7_agreement_between_render_and_has_overwrite() {
        // The same outcome that renders differently (Differs vs Fills) is what
        // has_overwrite reports.
        let diff = vec![("m".to_string(), makes_differs(5, 6))];
        assert!(has_overwrite(&diff));
        assert!(table(&diff).contains("differs"));

        let fill = vec![("m".to_string(), makes_fills(5, 6))];
        assert!(!has_overwrite(&fill));
        assert!(table(&fill).contains("fills"));
    }

    #[test]
    fn clause_8_input_order_preserved_exactly_once() {
        let pending = vec![
            ("second".to_string(), makes_fills(1, 2)),
            ("first".to_string(), makes_differs(3, 4)),
        ];
        let text = table(&pending);
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("second"));
        assert!(lines[1].contains("first"));
    }

    #[test]
    fn boundary_one_fills_exit_one_overwrite_false() {
        let pending = vec![("fill-one".to_string(), makes_fills(42, 99))];
        let mut buf = Vec::new();
        assert_eq!(run(&pending, &mut buf), 1);
        assert!(!has_overwrite(&pending));
    }

    #[test]
    fn boundary_one_differs_with_zero_prices_exit_one_overwrite_true() {
        let pending = vec![("zero-diff".to_string(), makes_differs(0, 0))];
        let mut buf = Vec::new();
        assert_eq!(run(&pending, &mut buf), 1);
        assert!(has_overwrite(&pending));
        // Must not collapse with Unquoted (absent) case.
        let unquoted = vec![("zero-unquoted".to_string(), Change::Unquoted)];
        assert!(!table(&unquoted).contains('0'));
        assert!(table(&pending).contains('0'));
    }

    #[test]
    fn boundary_duplicate_model_names_both_render() {
        let pending = vec![
            ("dup".to_string(), makes_differs(1, 2)),
            ("dup".to_string(), makes_fills(3, 4)),
        ];
        let text = table(&pending);
        assert_eq!(text.matches("dup").count(), 2);
    }

    #[test]
    fn boundary_max_u64_renders_as_number() {
        let pending = vec![("max".to_string(), makes_differs(u64::MAX, u64::MAX))];
        let text = table(&pending);
        assert!(text.contains(&u64::MAX.to_string()));
    }
}
