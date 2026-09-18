//! What one cell of the cross-examination matrix did, with the evidence a
//! VOID needs to name its own cause.
//!
//! Cross-examination runs every candidate's suite against every candidate's
//! code and records one cell per (suite, arm) pair. When the diagonal -- a
//! suite run against the code it shipped with -- fails to build, the whole
//! matrix is VOIDed, correctly: a broken transplant says nothing about
//! either candidate. What the harness retained until now was the single
//! word `nocompile`; the compiler's own explanation was discarded with the
//! temporary directory, and every VOID cost a separate hand investigation.
//! This module keeps the first compiler error line and classifies it well
//! enough that the next VOID names its own cause.
//!
//! This module owns the decision; [`crate::testout::failed_tests`] (imported
//! below) owns the reading of failing test names, exactly as it does for
//! [`crate::testout::classify`]. No I/O: the caller captures one
//! `cargo test` invocation's combined stdout+stderr and hands over the text.

use crate::testout::failed_tests;

/// Quoted as a [`Cell::NoCompile`]'s `first_error` when no line beginning
/// with `error` exists to quote: empty output, or a build that died (killed,
/// disk full) before rustc printed anything. It is a marker meaning "no
/// compiler line was captured", not an invented message.
const NO_OUTPUT: &str = "(no output)";

/// Build-failure markers, in classification precedence order: [`read_cell`]
/// scans top to bottom, so output matching several rows is classified by the
/// highest one. `could not compile` is deliberately last: on its own it is a
/// recognised build failure with no known cause, which is [`Breakage::Other`].
const BREAKAGE_MARKERS: &[(&str, Breakage)] = &[
    // The machine, not the code. These are checked before every code-level
    // marker because they usually arrive wrapped in `could not compile`.
    ("Disk quota exceeded", Breakage::Environment),
    ("No space left on device", Breakage::Environment),
    ("Killed", Breakage::Environment),
    // A graft that truncated or half-copied its target.
    (
        "this file contains an unclosed delimiter",
        Breakage::UnclosedDelimiter,
    ),
    // An import injected twice.
    ("error[E0252]", Breakage::DuplicateImport),
    // Real API divergence: the suite calls what this implementation lacks.
    ("error[E0599]", Breakage::MissingItem),
    ("error[E0425]", Breakage::MissingItem),
    ("error[E0433]", Breakage::MissingItem),
    // cargo's universal build-failure summary; the unclassified residue.
    ("could not compile", Breakage::Other),
];

/// What one cell of the matrix did, with the evidence.
///
/// CLOSED at these three variants. There is no "unreadable" state: output
/// that is recognisably neither a clean pass nor a run with named failures
/// is a cell that did not compile as far as this module can tell, and lands
/// in [`Cell::NoCompile`].
///
/// Note: two `Cell` enums already live in this crate, [`crate::crossx::Cell`]
/// (Pass/Fail/Error) and [`crate::matrix::Cell`] (Pass/Fail/NoCompile). Both
/// are unit-only summaries with nowhere to hold this module's contract --
/// failed test names, a classified breakage, and a verbatim compiler line
/// are data their variants cannot carry, and this module's brief fixes
/// exactly the shape below. Extending either existing enum would have broken
/// every exhaustive match on it in files this task may not touch, so the
/// name exists a third time, in the shape demanded -- the same trade
/// [`crate::testout::Outcome`] documented before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cell {
    /// The suite ran and every test passed.
    Pass,
    /// The suite ran and some tests failed.
    Fail {
        /// Failing test names, sorted and deduplicated, as read by
        /// [`crate::testout::failed_tests`]. Deliberately not the whole
        /// output: this module summarises what a caller should keep, and the
        /// caller decides whether to keep more.
        failed: Vec<String>,
    },
    /// The suite did not build.
    NoCompile {
        /// What went wrong, classified. Never a guess: a build failure this
        /// module does not recognise is [`Breakage::Other`].
        why: Breakage,
        /// The FIRST line beginning `error` in the output, verbatim --
        /// whitespace preserved after trimming the line's own trailing
        /// newline. Never empty: every path that builds a `NoCompile`
        /// supplies either a line found by that anchor (at least the five
        /// characters `error`) or [`NO_OUTPUT`], quoted when nothing in the
        /// output begins with `error`. A single line, not the whole output.
        ///
        /// This and `why` answer different questions -- "what did the
        /// compiler say first" and "what kind of loss was this" -- and are
        /// allowed to disagree: `why` follows the precedence order, the
        /// quoted line follows source order, so the first error shown may
        /// belong to a lower-precedence class than the one classified.
        first_error: String,
    },
}

/// Why a cell failed to build. A KNOWN SUBSET: every variant here was a real
/// matrix loss, and an unrecognised error is [`Breakage::Other`], never a
/// guess.
///
/// CLOSED at these five. The markers that trigger them are an OPEN subset:
/// rustc's wording and its error codes are not a stability guarantee, so
/// this module recognises exactly the markers listed on the variants and
/// refuses to guess past them. Both halves matter -- an unrecognised build
/// failure is still a build failure ([`Breakage::Other`], never
/// [`Cell::Pass`]), and it is never shoehorned into a classified variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Breakage {
    /// Unbalanced delimiters: the graft truncated or half-copied something.
    /// Marker: `this file contains an unclosed delimiter`.
    UnclosedDelimiter,
    /// The same name imported twice (E0252): the injection duplicated an
    /// import.
    DuplicateImport,
    /// A name the suite calls does not exist on this implementation (E0599,
    /// E0425, E0433). The one breakage that is a REAL API divergence rather
    /// than an instrument fault.
    MissingItem,
    /// The machine, not the code: no space, quota exceeded, killed.
    /// Recognised before every code-level marker, deliberately: a build that
    /// failed because the disk filled says nothing about either candidate,
    /// and these losses arrive wrapped in `could not compile` -- reporting
    /// the wrapper instead of the cause is how a full tmpfs was once read as
    /// an arm producing nothing.
    Environment,
    /// Recognised as a build failure (cargo's `could not compile` summary,
    /// or an unrecognised `error[...]` code), cause not classified.
    Other,
}

/// Classify one cell from a `cargo test` invocation's combined output.
///
/// Decision order, fixed here so it need not be rediscovered:
///
/// 1. A build-failure marker wins outright: any row of
///    [`BREAKAGE_MARKERS`] -- the environment markers, the unclosed
///    delimiter, an E0252/E0599/E0425/E0433 code, or cargo's `could not
///    compile` summary. This beats failing test names found in the same
///    output, because the build is the earlier fact and the names came from
///    a different crate in the same invocation; a crate that did not build
///    produced no evidence about its own implementation.
/// 2. Otherwise, if [`crate::testout::failed_tests`] finds any names, the
///    cell is [`Cell::Fail`] with those names, sorted and deduplicated.
/// 3. Otherwise, if the output contains `test result: ok.`, the cell is
///    [`Cell::Pass`].
/// 4. Otherwise, if any line begins with `error`, the cell is
///    [`Cell::NoCompile`] at [`Breakage::Other`] with that line: a build
///    failure this module's markers do not classify, never a pass.
/// 5. Otherwise -- including EMPTY output -- the cell is [`Cell::NoCompile`]
///    at [`Breakage::Environment`] with `first_error` `"(no output)"`. A
///    cell that produced no readable output did not compile and did not
///    run; the likeliest cause is the machine, and `Environment` is the
///    variant that indicts the harness.
///
/// The error-line stage sits after Pass and Fail on purpose: every suite
/// that built and ran ends its output with `error: test failed, to rerun
/// pass ...` when anything failed, and reading that as a build failure
/// would void every real failing cell. The anchor is the line start, so an
/// `error` that appears only inside a test name (`fn error_is_reported()`
/// prints as `test error_is_reported ... ok`) cannot trip it.
///
/// Precedence when several markers appear: `Environment`, then
/// `UnclosedDelimiter`, then `DuplicateImport`, then `MissingItem`, then
/// `Other` -- the order of [`BREAKAGE_MARKERS`]. Environment first because a
/// disk that filled mid-build implicates the machine before anything the
/// compiler said. The classification and `first_error` follow different
/// rules (precedence vs source order) and may disagree; see
/// [`Cell::NoCompile`].
pub fn read_cell(output: &str) -> Cell {
    if let Some(why) = classified_build_failure(output) {
        return Cell::NoCompile {
            why,
            first_error: first_error_line(output)
                .map(|line| line.to_string())
                .unwrap_or_else(|| NO_OUTPUT.to_string()),
        };
    }
    let failed = failed_tests(output);
    if !failed.is_empty() {
        return Cell::Fail { failed };
    }
    if output.contains("test result: ok.") {
        return Cell::Pass;
    }
    if let Some(line) = first_error_line(output) {
        return Cell::NoCompile {
            why: Breakage::Other,
            first_error: line.to_string(),
        };
    }
    Cell::NoCompile {
        why: Breakage::Environment,
        first_error: NO_OUTPUT.to_string(),
    }
}

/// Whether this breakage indicts the HARNESS rather than either candidate.
///
/// True for everything except [`Breakage::MissingItem`], which is the only
/// variant that says something about the code: a suite calling a method the
/// implementation never had is evidence about the transplant, and only
/// about the transplant.
pub fn is_instrument_fault(b: Breakage) -> bool {
    b != Breakage::MissingItem
}

/// The first marker from [`BREAKAGE_MARKERS`] present in `output`, if any.
fn classified_build_failure(output: &str) -> Option<Breakage> {
    BREAKAGE_MARKERS
        .iter()
        .find(|(marker, _)| output.contains(*marker))
        .map(|(_, why)| *why)
}

/// The first line of `output` beginning with `error`, verbatim. `str::lines`
/// has already trimmed the line's own trailing newline and nothing else.
fn first_error_line(output: &str) -> Option<&str> {
    output.lines().find(|line| line.starts_with("error"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASS: &str = "\
running 3 tests
test adjudicate::tests::rescues_once ... ok
test budget::tests::rejects_negative ... ok
test grading::tests::clamps_high ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
";

    // The `failures:` summary is deliberately out of alphabetical order and
    // `grading::tests::clamps_high` appears in both sections, to pin the
    // sorted-and-deduplicated composition of `Fail::failed`.
    const FAIL: &str = "\
running 4 tests
test roster::tests::spawns ... ok
test budget::tests::oversubscribes ... FAILED
test grading::tests::clamps_high ... FAILED
test budget::tests::denies_zero ... FAILED

failures:

---- grading::tests::clamps_high stdout ----
assertion `left <= right` failed
  left: 1.4
 right: 1.0

failures:

failures:
    grading::tests::clamps_high
    budget::tests::denies_zero
    budget::tests::oversubscribes

test result: FAILED. 1 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

error: test failed, to rerun pass `-p suite`
";

    // A run that reported ok for one crate while another's tests failed.
    const OK_AND_NAMED_FAILURES: &str = "\
running 5 tests
test alpha::tests::holds ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s

failures:

failures:
    beta::tests::explodes

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
";

    // The real loss: an import injected both into a braced group and into
    // the suite body.
    const DUPLICATE_IMPORT: &str = "\
error[E0252]: the name `BTreeMap` is defined multiple times
  --> crates/suite/src/lib.rs:4:5
   |
 3 | use std::collections::{BTreeMap, BTreeSet};
   |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ previous import of the type `BTreeMap` here
 4 | use std::collections::BTreeMap;
   |     ^^^^^^^^^^^^^^^^^^^^^^^^^^ `BTreeMap` reimported here
   |
error: could not compile `suite` (bin \"suite\") due to 1 previous error
";

    // The real loss: a strip that truncated a file on a QUOTED
    // `"#[cfg(test)]"` and cut the closing brace with it.
    const UNCLOSED_DELIMITER: &str = "\
error: this file contains an unclosed delimiter
  --> crates/suite/src/lib.rs:31:38
   |
12 |   fn graftted_tests() {
   |  ______________________^
31 | | }
   | |_^
error: could not compile `suite` (bin \"suite\") due to 1 previous error
";

    const MISSING_ITEM: &str = "\
error[E0599]: no method named `probe_slot` found for struct `Matrix` in the current scope
  --> crates/suite/src/lib.rs:88:10
   |
88 |     m.probe_slot(0, 1);
   |      ^^^^^^^^^^ method not found in `Matrix`
   |
error: could not compile `suite` (lib) due to 1 previous error
";

    // The real loss: a full tmpfs. The env marker arrives AFTER (and
    // indented under) cargo's `could not compile`, as it always does.
    const DISK_QUOTA: &str = "\
error: could not compile `suite` (bin \"suite\")

Caused by:
  process didn't exit successfully: rustc --crate-name suite --edition=2024 ... (exit status: 1)
  --- stderr
  error: failed to write `/tmp/fb-matrix/target/debug/deps/libsuite.rlib`: Disk quota exceeded (os error 122)
";

    const NO_SPACE: &str = "\
error: linking with `cc` failed: No space left on device (os error 28)
  |
  = note: ld: final link failed: No space left on device
error: could not compile `suite` (bin \"suite\") due to 1 previous error
";

    // A failing run whose names could not be parsed but whose trailing
    // cargo line is an error line: the spec's Other-when-an-error-line case.
    const FAILED_NO_NAMES_WITH_ERROR: &str = "\
running 2 tests
test graftted::suite_a ... FAILED
test graftted::suite_b ... FAILED

test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

error: test failed, to rerun pass `-p suite`
";

    // Same shape, truncated before cargo's trailing error line: the spec's
    // otherwise-case.
    const FAILED_NO_NAMES_NO_ERROR: &str = "\
running 2 tests
test graftted::suite_a ... FAILED
test graftted::suite_b ... FAILED

test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
";

    // A failures block from another crate in the same invocation, plus that
    // other crate's compile failure.
    const OTHER_CRATE_FAILURES: &str = "\
running 5 tests
test alpha::tests::holds ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s

failures:

failures:
    beta::tests::explodes

error: could not compile `beta` (bin \"beta\") due to 1 previous error
";

    #[test]
    fn all_pass_is_a_pass() {
        assert_eq!(read_cell(PASS), Cell::Pass);
    }

    #[test]
    fn named_failures_are_sorted_and_deduplicated() {
        assert_eq!(
            read_cell(FAIL),
            Cell::Fail {
                failed: vec![
                    "budget::tests::denies_zero".to_string(),
                    "budget::tests::oversubscribes".to_string(),
                    "grading::tests::clamps_high".to_string(),
                ],
            }
        );
    }

    #[test]
    fn an_ok_marker_does_not_outvote_named_failures() {
        assert_eq!(
            read_cell(OK_AND_NAMED_FAILURES),
            Cell::Fail {
                failed: vec!["beta::tests::explodes".to_string()],
            }
        );
    }

    #[test]
    fn duplicated_import_is_classified_with_its_first_line() {
        assert_eq!(
            read_cell(DUPLICATE_IMPORT),
            Cell::NoCompile {
                why: Breakage::DuplicateImport,
                first_error: "error[E0252]: the name `BTreeMap` is defined multiple times"
                    .to_string(),
            }
        );
    }

    #[test]
    fn truncated_graft_is_an_unclosed_delimiter() {
        assert_eq!(
            read_cell(UNCLOSED_DELIMITER),
            Cell::NoCompile {
                why: Breakage::UnclosedDelimiter,
                first_error: "error: this file contains an unclosed delimiter".to_string(),
            }
        );
    }

    #[test]
    fn missing_method_is_a_missing_item() {
        assert_eq!(
            read_cell(MISSING_ITEM),
            Cell::NoCompile {
                why: Breakage::MissingItem,
                first_error:
                    "error[E0599]: no method named `probe_slot` found for struct `Matrix` in the current scope"
                        .to_string(),
            }
        );
    }

    #[test]
    fn every_missing_item_code_classifies() {
        for line in [
            "error[E0425]: cannot find value `slots` in this scope",
            "error[E0433]: failed to resolve: use of undeclared crate or module `witness`",
        ] {
            assert_eq!(
                read_cell(&format!(
                    "{line}\nerror: could not compile `suite` due to 1 previous error\n"
                )),
                Cell::NoCompile {
                    why: Breakage::MissingItem,
                    first_error: line.to_string(),
                }
            );
        }
    }

    #[test]
    fn full_disk_is_environment_even_though_it_says_could_not_compile() {
        assert_eq!(
            read_cell(DISK_QUOTA),
            Cell::NoCompile {
                why: Breakage::Environment,
                first_error: "error: could not compile `suite` (bin \"suite\")".to_string(),
            }
        );
    }

    #[test]
    fn every_environment_marker_classifies() {
        // `Killed` arrives with no compiler line at all, so `first_error` is
        // the no-compiler-line sentinel; only the classification is pinned.
        for output in ["Killed", NO_SPACE] {
            assert!(
                matches!(
                    read_cell(output),
                    Cell::NoCompile {
                        why: Breakage::Environment,
                        ..
                    }
                ),
                "not Environment: {output:?}"
            );
        }
    }

    #[test]
    fn environment_outranks_unclosed_delimiter_but_the_quote_follows_source_order() {
        let output = "error: this file contains an unclosed delimiter\nKilled\n";
        assert_eq!(
            read_cell(output),
            Cell::NoCompile {
                why: Breakage::Environment,
                first_error: "error: this file contains an unclosed delimiter".to_string(),
            }
        );
    }

    #[test]
    fn unclosed_delimiter_outranks_duplicate_import() {
        let output = "error[E0252]: the name `BTreeMap` is defined multiple times\n\
                      error: this file contains an unclosed delimiter\n";
        assert_eq!(
            read_cell(output),
            Cell::NoCompile {
                why: Breakage::UnclosedDelimiter,
                first_error: "error[E0252]: the name `BTreeMap` is defined multiple times"
                    .to_string(),
            }
        );
    }

    #[test]
    fn duplicate_import_outranks_missing_item() {
        let output = "error[E0425]: cannot find value `slots` in this scope\n\
                      error[E0252]: the name `BTreeMap` is defined multiple times\n";
        assert_eq!(
            read_cell(output),
            Cell::NoCompile {
                why: Breakage::DuplicateImport,
                first_error: "error[E0425]: cannot find value `slots` in this scope".to_string(),
            }
        );
    }

    #[test]
    fn empty_output_is_an_environment_loss_with_the_sentinel() {
        assert_eq!(
            read_cell(""),
            Cell::NoCompile {
                why: Breakage::Environment,
                first_error: "(no output)".to_string(),
            }
        );
    }

    #[test]
    fn error_inside_a_test_name_is_not_a_build_failure() {
        let output = "\
running 2 tests
test error_is_reported ... ok
test reports_error ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
";
        assert_eq!(read_cell(output), Cell::Pass);
    }

    #[test]
    fn an_error_printed_by_a_failing_test_does_not_void_the_names() {
        let output = "\
running 1 test
test error_scan ... FAILED

failures:

---- error_scan stdout ----
error: expected 4 exits, got 3

failures:
    error_scan

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

error: test failed, to rerun pass `-p suite`
";
        assert_eq!(
            read_cell(output),
            Cell::Fail {
                failed: vec!["error_scan".to_string()],
            }
        );
    }

    #[test]
    fn failed_run_with_unparseable_names_and_an_error_line_is_other() {
        assert_eq!(
            read_cell(FAILED_NO_NAMES_WITH_ERROR),
            Cell::NoCompile {
                why: Breakage::Other,
                first_error: "error: test failed, to rerun pass `-p suite`".to_string(),
            }
        );
    }

    #[test]
    fn failed_run_with_unparseable_names_and_no_error_line_is_environment() {
        assert_eq!(
            read_cell(FAILED_NO_NAMES_NO_ERROR),
            Cell::NoCompile {
                why: Breakage::Environment,
                first_error: "(no output)".to_string(),
            }
        );
    }

    #[test]
    fn another_crates_failures_do_not_survive_a_could_not_compile() {
        assert_eq!(
            read_cell(OTHER_CRATE_FAILURES),
            Cell::NoCompile {
                why: Breakage::Other,
                first_error:
                    "error: could not compile `beta` (bin \"beta\") due to 1 previous error"
                        .to_string(),
            }
        );
    }

    #[test]
    fn an_unrecognised_error_code_is_other_and_never_a_pass() {
        let output = "error[E0308]: mismatched types\n --> crates/suite/src/lib.rs:9:9\n";
        assert_eq!(
            read_cell(output),
            Cell::NoCompile {
                why: Breakage::Other,
                first_error: "error[E0308]: mismatched types".to_string(),
            }
        );
    }

    #[test]
    fn first_error_is_verbatim_including_trailing_whitespace() {
        let output = "error[E0433]: failed to resolve: could not find `grid` in `witness`   \n\
                      error: could not compile `suite` due to 1 previous error\n";
        assert_eq!(
            read_cell(output),
            Cell::NoCompile {
                why: Breakage::MissingItem,
                first_error:
                    "error[E0433]: failed to resolve: could not find `grid` in `witness`   "
                        .to_string(),
            }
        );
    }

    #[test]
    fn only_missing_item_indicts_the_code() {
        assert!(is_instrument_fault(Breakage::UnclosedDelimiter));
        assert!(is_instrument_fault(Breakage::DuplicateImport));
        assert!(is_instrument_fault(Breakage::Environment));
        assert!(is_instrument_fault(Breakage::Other));
        assert!(!is_instrument_fault(Breakage::MissingItem));
    }
}




