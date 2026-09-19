//! The gatherer between `sources.toml`, a provider price catalogue and
//! [`crate::prices_cmd`] — the [`crate::compare_gather`] layer for prices.
//!
//! `fb prices` currently passes an empty slice, so "nothing pending", exit 0
//! means both "every registered price agrees" and "nobody looked". This
//! module reads the registry and a catalogue and produces the real list, so
//! an unreadable source can be reported as exit 4 instead.
//!
//! Two decisions are worth stating because the specification is divided on
//! them:
//!
//! - The list is walked per arm through `price_update::compare`, not through
//!   `price_update::pending`. `pending` omits `Change::Unquoted` arms by
//!   contract, which would make a catalogue that lists no registered model
//!   read as a real all-clear; the spec requires the opposite (exit 1, no
//!   zero figure). On the edits (`Differs`, `Fills`) the two agree entry for
//!   entry — no price arithmetic lives in this file.
//! - Prices in both files are stated per 1M tokens and scaled by
//!   [`MICRO_UNITS`] into the micro-unit `u64`s `Registered` and `Quoted`
//!   store, identically on both sides, so agreement is exact whatever unit a
//!   caller writes. A missing catalogue field makes that entry absent from
//!   the quoted list, never a malformed file.
//!
//! Unwired from the argument parser by design: wiring is a separate task.
//! Until then the public functions are reachable but unused, which trips
//! `dead_code` in a binary crate, so the module suppresses it.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use farmerbob_core::measurement::Measurement;
use farmerbob_core::price_update::{self, Change, Quoted, Registered};
use serde::Deserialize;

use crate::prices_cmd;

/// Micro-units per stated unit: `price_update` stores prices in millionths
/// so its types can stay `Eq`.
const MICRO_UNITS: f64 = 1_000_000.0;

/// Why the comparison could not be made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatherError {
    /// A file does not exist or could not be read. Carries which, and a reason.
    Unreadable {
        /// The file that could not be read, as the caller spelled it.
        path: String,
        /// What went wrong, in prose.
        reason: String,
    },
    /// A file parsed but is not the shape expected. Carries which, and a reason.
    Malformed {
        /// The file whose shape is wrong, as the caller spelled it.
        path: String,
        /// What is wrong with the shape, in prose.
        reason: String,
    },
}

/// One `[source.<name>]` table, read as exactly the three price fields.
#[derive(Deserialize)]
struct SourceEntry {
    /// The model id the arm runs. A source without one is skipped.
    model: Option<String>,
    /// The registered input price, as written in the file.
    price_in: Option<toml::Value>,
    /// The registered output price, as written in the file.
    price_out: Option<toml::Value>,
}

/// The registry file: every `[source.<name>]` table, keyed by name.
#[derive(Deserialize)]
struct SourcesFile {
    /// The `[source]` table; an empty or absent one registers nothing.
    source: Option<BTreeMap<String, SourceEntry>>,
}

/// Read the registry and a catalogue into the pending-change list.
///
/// Entries carry the arm's registry name and appear in the registry's order,
/// extended with catalogue-missing arms so an unquoted model can never read
/// as free. The comparison itself is `price_update`'s.
pub fn pending_from(
    sources_toml: &Path,
    catalogue_json: &Path,
) -> Result<Vec<(String, Change)>, GatherError> {
    let registered = read_registry(sources_toml)?;
    let catalogue = read_catalogue(catalogue_json)?;
    Ok(pending_list(&registered, &catalogue))
}

/// Read both sources and render. Returns the exit code the caller should use.
///
/// On a successful read the code is [`crate::prices_cmd::run`]'s, unchanged.
/// On a read failure the code is 4, `prices_cmd` is never called, and one
/// diagnostic line naming the file goes to `out` — the
/// [`crate::compare_gather::run`] pattern.
pub fn run(sources_toml: &Path, catalogue_json: &Path, out: &mut dyn Write) -> i32 {
    match pending_from(sources_toml, catalogue_json) {
        Ok(pending) => prices_cmd::run(&pending, out),
        Err(err) => {
            let (path, kind, reason) = match err {
                GatherError::Unreadable { path, reason } => (path, "unreadable", reason),
                GatherError::Malformed { path, reason } => (path, "malformed", reason),
            };
            let _ = writeln!(out, "{path}: {kind}: {reason}");
            4
        }
    }
}

/// Read `sources.toml` into the arms `price_update` compares.
fn read_registry(path: &Path) -> Result<Vec<Registered>, GatherError> {
    let text = std::fs::read_to_string(path).map_err(|e| GatherError::Unreadable {
        path: path.display().to_string(),
        reason: e.to_string(),
    })?;
    let file: SourcesFile = toml::from_str(&text).map_err(|e| GatherError::Malformed {
        path: path.display().to_string(),
        reason: e.to_string(),
    })?;

    let mut registered = Vec::new();
    for (arm, entry) in file.source.unwrap_or_default() {
        let Some(model) = entry.model else {
            continue;
        };
        let price_in = registry_price(&entry.price_in, path)?;
        let price_out = registry_price(&entry.price_out, path)?;
        registered.push(Registered {
            arm,
            model,
            price_in,
            price_out,
        });
    }
    Ok(registered)
}

/// One registry price: absent means absent (`Fills` exists for that), a
/// non-number or an unusable number is a shape error.
fn registry_price(
    written: &Option<toml::Value>,
    path: &Path,
) -> Result<Measurement<u64>, GatherError> {
    let Some(value) = written else {
        return Ok(Measurement::not_attempted());
    };
    let stated = match value {
        toml::Value::Integer(i) => *i as f64,
        toml::Value::Float(f) => *f,
        _ => {
            return Err(GatherError::Malformed {
                path: path.display().to_string(),
                reason: "price is not a number".to_string(),
            });
        }
    };
    match to_micro_units(stated) {
        Some(micro) => Ok(Measurement::observed(micro)),
        None => Err(GatherError::Malformed {
            path: path.display().to_string(),
            reason: "price is negative or out of range".to_string(),
        }),
    }
}

/// Read the catalogue JSON into `price_update`'s quoted prices. An entry
/// missing any pinned field — or carrying one this module cannot use — is
/// absent from the quoted list, not a malformed file.
fn read_catalogue(path: &Path) -> Result<Vec<Quoted>, GatherError> {
    let text = std::fs::read_to_string(path).map_err(|e| GatherError::Unreadable {
        path: path.display().to_string(),
        reason: e.to_string(),
    })?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| GatherError::Malformed {
            path: path.display().to_string(),
            reason: e.to_string(),
        })?;
    let Some(entries) = value.as_array() else {
        return Err(GatherError::Malformed {
            path: path.display().to_string(),
            reason: "top-level JSON value is not an array".to_string(),
        });
    };

    let mut catalogue = Vec::new();
    for entry in entries {
        if let Some(quoted) = quoted_entry(entry) {
            catalogue.push(quoted);
        }
    }
    Ok(catalogue)
}

/// One catalogue entry, or `None` when the entry cannot be quoted.
fn quoted_entry(entry: &serde_json::Value) -> Option<Quoted> {
    let object = entry.as_object()?;
    let model = object.get("model")?.as_str()?;
    let price_in = to_micro_units(object.get("price_in")?.as_f64()?)?;
    let price_out = to_micro_units(object.get("price_out")?.as_f64()?)?;
    Some(Quoted {
        model: model.to_string(),
        price_in,
        price_out,
    })
}

/// Every arm whose outcome is not `Agrees`, in registry order.
///
/// Each outcome is `price_update::compare`'s; on the edits this is exactly
/// the list `price_update::pending` returns, plus the `Unquoted` arms
/// `pending` does not report.
fn pending_list(registered: &[Registered], catalogue: &[Quoted]) -> Vec<(String, Change)> {
    registered
        .iter()
        .filter_map(|reg| match price_update::compare(reg, catalogue) {
            Change::Agrees => None,
            change => Some((reg.arm.clone(), change)),
        })
        .collect()
}

/// A stated price to the micro-unit `u64` the comparison stores. `None` when
/// the value is negative, not finite, or too large to hold.
fn to_micro_units(stated: f64) -> Option<u64> {
    if !stated.is_finite() || stated < 0.0 {
        return None;
    }
    let micro = stated * MICRO_UNITS;
    if micro >= u64::MAX as f64 {
        return None;
    }
    Some(micro.round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static FIXTURE_COUNTER: AtomicU32 = AtomicU32::new(0);

    fn stamp() -> String {
        format!(
            "{}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default(),
            FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed),
        )
    }

    /// Write a fixture under the temp dir. The repository's real
    /// `sources.toml` is live configuration and is never read.
    fn fixture(name: &str, contents: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("fb_prices_gather_test_{}_{}", stamp(), name));
        std::fs::write(&path, contents).unwrap();
        path
    }

    /// A missing path with a unique name, for the unreadable cases.
    fn missing(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("fb_prices_gather_absent_{}_{}", stamp(), name))
    }

    const ONE_ARM: &str = "\
[source.arm-alpha]
model = \"vendor/alpha\"
price_in = 1
price_out = 2
";

    /// Clause 1 and clause 5 as a pair: the all-clear and the unreadable
    /// registry must never be confusable.
    #[test]
    fn clause_1_and_5_agrees_exit_zero_unreadable_exit_four() {
        let sources = fixture("sources_agree.toml", ONE_ARM);
        let catalogue = fixture(
            "catalogue_agree.json",
            "[{\"model\": \"vendor/alpha\", \"price_in\": 1, \"price_out\": 2}]",
        );

        assert_eq!(pending_from(&sources, &catalogue), Ok(vec![]));
        let mut out = Vec::new();
        assert_eq!(run(&sources, &catalogue, &mut out), 0);

        let gone = missing("sources.toml");
        match pending_from(&gone, &catalogue) {
            Err(GatherError::Unreadable { path, reason }) => {
                assert_eq!(path, gone.display().to_string());
                assert!(!reason.is_empty());
            }
            other => panic!("expected Unreadable naming the registry, got {other:?}"),
        }
        let mut out = Vec::new();
        assert_eq!(run(&gone, &catalogue, &mut out), 4);
    }

    /// Clause 6: the catalogue error names the catalogue, distinct from the
    /// registry path clause 5 names.
    #[test]
    fn clause_6_missing_catalogue_names_the_catalogue() {
        let sources = fixture("sources_one.toml", ONE_ARM);
        let gone = missing("catalogue.json");

        match pending_from(&sources, &gone) {
            Err(GatherError::Unreadable { path, reason }) => {
                assert_eq!(path, gone.display().to_string());
                assert!(!reason.is_empty());
            }
            other => panic!("expected Unreadable naming the catalogue, got {other:?}"),
        }
        let mut out = Vec::new();
        assert_eq!(run(&sources, &gone, &mut out), 4);
    }

    /// Clause 2: one entry, `Differs`, exit 1.
    #[test]
    fn clause_2_difference_is_one_differs_entry_exit_one() {
        let sources = fixture("sources_differs.toml", ONE_ARM);
        let catalogue = fixture(
            "catalogue_differs.json",
            "[{\"model\": \"vendor/alpha\", \"price_in\": 5, \"price_out\": 6}]",
        );

        let pending = pending_from(&sources, &catalogue).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, "arm-alpha");
        assert!(matches!(pending[0].1, Change::Differs { .. }));

        let mut out = Vec::new();
        assert_eq!(run(&sources, &catalogue, &mut out), 1);
    }

    /// Clause 3: a registered arm with no price, quoted, is `Fills`, exit 1.
    #[test]
    fn clause_3_unpriced_and_quoted_is_fills_exit_one() {
        let sources = fixture(
            "sources_unpriced.toml",
            "[source.arm-beta]\nmodel = \"vendor/beta\"\n",
        );
        let catalogue = fixture(
            "catalogue_quotes_beta.json",
            "[{\"model\": \"vendor/beta\", \"price_in\": 3, \"price_out\": 4}]",
        );

        let pending = pending_from(&sources, &catalogue).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, "arm-beta");
        assert!(matches!(pending[0].1, Change::Fills { .. }));

        let mut out = Vec::new();
        assert_eq!(run(&sources, &catalogue, &mut out), 1);
    }

    /// Clause 4 and the empty-catalogue boundary: a registered arm absent
    /// from the catalogue is pending, exit 1, and never renders as free.
    #[test]
    fn clause_4_unquoted_is_pending_exit_one_with_no_zero_figure() {
        let sources = fixture("sources_unquoted.toml", ONE_ARM);
        let catalogue = fixture("catalogue_empty.json", "[]");

        let pending = pending_from(&sources, &catalogue).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, "arm-alpha");
        assert_eq!(pending[0].1, Change::Unquoted);

        let mut out = Vec::new();
        assert_eq!(run(&sources, &catalogue, &mut out), 1);
        let text = String::from_utf8(out).unwrap();
        assert!(
            !text.contains('0'),
            "an unquoted model must not render as free: {text:?}"
        );
    }

    /// Clauses 7 and 8, plus the shape rule the `Malformed` doc pins: a file
    /// that parses but is not the expected shape.
    #[test]
    fn clause_7_and_8_malformed_shapes_exit_four() {
        let sources = fixture("sources_one.toml", ONE_ARM);
        let not_array = fixture("catalogue_object.json", "{}");
        assert!(matches!(
            pending_from(&sources, &not_array),
            Err(GatherError::Malformed { path, .. }) if path == not_array.display().to_string()
        ));
        let mut out = Vec::new();
        assert_eq!(run(&sources, &not_array, &mut out), 4);

        let broken = fixture("sources_broken.toml", "not valid toml ][");
        let catalogue = fixture("catalogue_empty_two.json", "[]");
        assert!(matches!(
            pending_from(&broken, &catalogue),
            Err(GatherError::Malformed { path, .. }) if path == broken.display().to_string()
        ));

        let wrong_shape = fixture("sources_wrong_shape.toml", "source = 3");
        assert!(matches!(
            pending_from(&wrong_shape, &catalogue),
            Err(GatherError::Malformed { path, .. }) if path == wrong_shape.display().to_string()
        ));
    }

    /// Boundary: an empty registry (an empty file, or sources without a
    /// model) against a readable catalogue is a real all-clear.
    #[test]
    fn boundary_empty_registry_is_all_clear() {
        let catalogue = fixture(
            "catalogue_any.json",
            "[{\"model\": \"vendor/alpha\", \"price_in\": 1, \"price_out\": 2}]",
        );

        let empty = fixture("sources_empty.toml", "");
        assert_eq!(pending_from(&empty, &catalogue), Ok(vec![]));
        let mut out = Vec::new();
        assert_eq!(run(&empty, &catalogue, &mut out), 0);

        let no_model = fixture("sources_no_model.toml", "[source.arm-alpha]\n");
        assert_eq!(pending_from(&no_model, &catalogue), Ok(vec![]));
    }

    /// Clauses 9 and 10: the edits come back in registry order under the arm
    /// names `pending` reports, and agree with `pending` fed the same
    /// figures.
    #[test]
    fn clause_9_and_10_order_and_agreement_with_pending() {
        let sources = fixture(
            "sources_mixed.toml",
            "\
[source.arm-a]
model = \"vendor/a\"
price_in = 1
price_out = 2

[source.arm-b]
model = \"vendor/b\"

[source.arm-c]
model = \"vendor/c\"
price_in = 7
price_out = 8
",
        );
        let catalogue = fixture(
            "catalogue_mixed.json",
            "[\
{\"model\": \"vendor/a\", \"price_in\": 9, \"price_out\": 9},\
 {\"model\": \"vendor/b\", \"price_in\": 3, \"price_out\": 4},\
 {\"model\": \"vendor/c\", \"price_in\": 7, \"price_out\": 8}]",
        );

        let pending = pending_from(&sources, &catalogue).unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].0, "arm-a");
        assert!(matches!(pending[0].1, Change::Differs { .. }));
        assert_eq!(pending[1].0, "arm-b");
        assert!(matches!(pending[1].1, Change::Fills { .. }));

        // The same figures at `price_update`'s level produce the same edit
        // list — variant for variant, arm for arm.
        let regs = [
            Registered {
                arm: "arm-a".to_string(),
                model: "vendor/a".to_string(),
                price_in: Measurement::observed(1),
                price_out: Measurement::observed(2),
            },
            Registered {
                arm: "arm-b".to_string(),
                model: "vendor/b".to_string(),
                price_in: Measurement::not_attempted(),
                price_out: Measurement::not_attempted(),
            },
        ];
        let cat = [Quoted {
            model: "vendor/a".to_string(),
            price_in: 9,
            price_out: 9,
        }];
        let oracle = price_update::pending(&regs, &cat);
        assert_eq!(oracle.len(), 1);
        assert_eq!(oracle[0].0, "arm-a");
        assert!(matches!(oracle[0].1, Change::Differs { .. }));
    }

    /// A catalogue entry missing a pinned field is absent from the quoted
    /// list, so the model reads as catalogue-missing — not as free.
    #[test]
    fn catalogue_entry_missing_a_field_is_absent_not_malformed() {
        let sources = fixture("sources_field.toml", ONE_ARM);
        let catalogue = fixture(
            "catalogue_partial.json",
            "[{\"model\": \"vendor/alpha\", \"price_in\": 1}]",
        );

        let pending = pending_from(&sources, &catalogue).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].1, Change::Unquoted);

        let mut out = Vec::new();
        assert_eq!(run(&sources, &catalogue, &mut out), 1);
        let text = String::from_utf8(out).unwrap();
        assert!(!text.contains('0'), "must not render as free: {text:?}");
    }
}
