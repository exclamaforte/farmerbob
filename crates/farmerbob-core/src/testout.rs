//! Turning raw `cargo test` output into evidence [`witness::read`] can use.
//!
//! Cross-examination runs a suite against a field of implementations and
//! records one cell per (suite, arm) pair. If that cell keeps only pass/fail,
//! everything that separates "one shared disagreement" from "several
//! independent defects" is thrown away before [`witness::read`] ever sees it:
//! both look identical as a single boolean. This module reads the test NAMES
//! out of a run's combined stdout+stderr so that evidence survives.
//!
//! Three things can happen to a `cargo test` invocation, and they must not be
//! confused: it can run and report specific failing tests, it can fail to
//! build at all (no test ran, so no test names exist), or its output can be
//! something this module does not recognise. Only the first of those may ever
//! produce a [`Witness`]; the other two must say so honestly rather than
//! report zero failures, which would read as "everything passed."
//!
//! [`witness::read`]: crate::witness::read

use crate::witness::Witness;
use std::collections::BTreeSet;

/// Names of the tests that failed in one `cargo test` run, sorted and
/// deduplicated.
///
/// Empty means the run had no failing tests, which is NOT the same as the
/// run having failed to build -- see [`Outcome`].
///
/// Names are read from two places in `cargo test`'s output and unioned:
///
/// 1. The indented lines under a line that is exactly `failures:` (cargo
///    prints this list once, near the end, after the per-test result lines).
/// 2. The `---- <path> stdout ----` header that precedes each failing test's
///    captured output.
///
/// Reading both and deduplicating makes the result independent of which
/// section a given cargo version emits, and independent of `cargo` printing
/// the same name in both sections. Names are taken verbatim -- including
/// module prefixes and any prefix a test-graft process adds -- because two
/// implementations' suites are compared by name, and a normalisation that
/// collapses two distinct names would manufacture a false agreement.
pub fn failed_tests(output: &str) -> Vec<String> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    let lines: Vec<&str> = output.lines().collect();

    // Source 1: indented paths under a bare "failures:" line. A cargo run
    // prints this line twice -- once immediately before the captured-output
    // blocks (with nothing indented under it) and once as the final summary
    // (with one indented path per failing test) -- so scanning every
    // occurrence and only keeping what is actually indented underneath it
    // naturally picks up the summary and skips the empty one.
    let mut index = 0;
    while index < lines.len() {
        if lines[index].trim_end() == "failures:" {
            let mut cursor = index + 1;
            while cursor < lines.len() {
                let line = lines[cursor];
                let is_indented = line.starts_with(' ') || line.starts_with('\t');
                if is_indented && !line.trim().is_empty() {
                    names.insert(line.trim().to_string());
                    cursor += 1;
                } else {
                    break;
                }
            }
            index = cursor;
        } else {
            index += 1;
        }
    }

    // Source 2: "---- <path> stdout ----" headers, one per failing test.
    for line in &lines {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("---- ")
            && let Some(path) = rest.strip_suffix(" stdout ----")
            && !path.is_empty()
        {
            names.insert(path.to_string());
        }
    }

    names.into_iter().collect()
}

/// What a `cargo test` run actually did.
///
/// A closed set of four. The markers [`classify`] looks for -- `failures:`,
/// `---- ... stdout ----`, `test result: ok.`, `test result: FAILED`,
/// `could not compile`, `error[E....]:` -- are cargo's output shape, not a
/// stability guarantee; a shape this module does not recognise always falls
/// to [`Outcome::Unreadable`] and never to [`Outcome::Passed`].
///
/// Note: this name is also used, for an unrelated concept, by
/// [`crate::outcome::Outcome`] (the measured result of one arm attempting one
/// task -- a struct with a verdict, a class, a rescue level, and cost
/// fields). That type cannot express what this module needs: it describes a
/// whole task attempt, not the shape of one `cargo test` invocation's raw
/// output, and it is a struct where this is a closed enum over four
/// mutually-exclusive run shapes. Reusing it would mean bolting these four
/// variants onto unrelated fields, which is a bigger defect than the
/// duplicate name. See the handoff for the full note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Every test passed.
    Passed,
    /// The suite ran and these tests failed.
    Failed {
        /// Sorted, deduplicated names.
        failed: Vec<String>,
    },
    /// The suite did not build. No test names are available and none must be
    /// invented: an empty failure list here would read as "nothing failed".
    DidNotCompile,
    /// The output could not be classified. Distinct from every other variant:
    /// a reader must be able to tell "I could not parse this" from "it
    /// passed".
    Unreadable,
}

/// Classify one run's combined stdout+stderr.
///
/// Decision order, fixed here so it need not be rediscovered:
///
/// 1. A compile-failure marker (`could not compile`, or `error[E` followed
///    by digits and `]:`) wins outright and yields [`Outcome::DidNotCompile`],
///    even if the output also contains a `failures:` block -- for instance
///    from an earlier crate that built and ran in the same invocation. A
///    crate that did not build produced no evidence about its own
///    implementation, and reporting another crate's failing names against it
///    would attribute one crate's failures to a different one.
/// 2. Otherwise, if [`failed_tests`] finds any names, the run is
///    [`Outcome::Failed`] with those names. This also covers the case of
///    several crates in one invocation where one passed and one failed: the
///    input is one cell's output and the caller ran one crate, so the failing
///    names are simply returned without attempting to attribute them to a
///    particular crate.
/// 3. Otherwise, if the output contains `test result: ok.`, the run is
///    [`Outcome::Passed`].
/// 4. Otherwise the run is [`Outcome::Unreadable`]. This also covers output
///    that reports `test result: FAILED` but from which no test names could
///    be parsed: an empty failure list would misrepresent a failed run as a
///    clean one, so this module refuses to report one on missing evidence.
pub fn classify(output: &str) -> Outcome {
    if has_compile_error_marker(output) {
        return Outcome::DidNotCompile;
    }
    let failed = failed_tests(output);
    if !failed.is_empty() {
        return Outcome::Failed { failed };
    }
    if output.contains("test result: ok.") {
        return Outcome::Passed;
    }
    Outcome::Unreadable
}

/// Whether `output` carries a marker that the compilation itself failed.
///
/// `could not compile` is matched literally. `error[E....]:` is matched
/// structurally (`error[E` followed by one or more ASCII digits then `]:`)
/// rather than against a fixed code list, since new codes are added to rustc
/// over time and this module must not need to learn each one by name.
fn has_compile_error_marker(output: &str) -> bool {
    if output.contains("could not compile") {
        return true;
    }
    output.match_indices("error[E").any(|(start, marker)| {
        let after = &output[start + marker.len()..];
        let digit_len = after
            .as_bytes()
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        digit_len > 0 && after[digit_len..].starts_with("]:")
    })
}

/// Build the value `witness::read` consumes.
///
/// Returns `None` when the outcome carries no test names to report --
/// [`Outcome::DidNotCompile`] and [`Outcome::Unreadable`] -- because a
/// `Witness` with an empty `failed` list asserts that the suite passed, and
/// this module must never assert that on missing evidence.
pub fn witness_for(impl_arm: &str, output: &str) -> Option<Witness> {
    match classify(output) {
        Outcome::Passed => Some(Witness {
            impl_arm: impl_arm.to_string(),
            failed: Vec::new(),
        }),
        Outcome::Failed { failed } => Some(Witness {
            impl_arm: impl_arm.to_string(),
            failed,
        }),
        Outcome::DidNotCompile | Outcome::Unreadable => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A realistic multi-failure run: two failing tests among one passer, with
    // both the captured-output headers and the final summary list present
    // and agreeing.
    const TWO_FAILURES_BOTH_SECTIONS: &str = "\
running 3 tests
test quota::tests::succeeded_resets_attempts ... ok
test marker::tests::tab_mixed_with_spaces_is_still_a_tab ... FAILED
test marker::tests::precedence_violation ... FAILED

failures:

---- marker::tests::tab_mixed_with_spaces_is_still_a_tab stdout ----
thread 'marker::tests::tab_mixed_with_spaces_is_still_a_tab' panicked at src/marker.rs:42:5:
assertion `left == right` failed
  left: false
 right: true
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

---- marker::tests::precedence_violation stdout ----
thread 'marker::tests::precedence_violation' panicked at src/marker.rs:88:9:
assertion failed: precedence


failures:
    marker::tests::precedence_violation
    marker::tests::tab_mixed_with_spaces_is_still_a_tab

test result: FAILED. 1 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
";

    // A shape that omits the "failures:" summary entirely and reports only
    // the captured-output headers -- pinning clause 2's second half.
    const ONLY_HEADERS: &str = "\
running 1 test
test xtests_0::availability::cell_3_1 ... FAILED

---- xtests_0::availability::cell_3_1 stdout ----
thread 'xtests_0::availability::cell_3_1' panicked at src/availability.rs:10:5:
unpinned clause

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
";

    const SINGLE_FAILURE: &str = "\
running 1 test
test tests::single_fault ... FAILED

failures:

---- tests::single_fault stdout ----
thread 'tests::single_fault' panicked at src/lib.rs:1:1:
boom

failures:
    tests::single_fault

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
";

    const ALL_PASSED: &str = "\
running 3 tests
test foo::bar ... ok
test foo::baz ... ok
test foo::qux ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
";

    // A compile failure from a second crate in the same invocation, after an
    // earlier crate's tests already ran and left a populated failures block.
    // Pins clause 5: DidNotCompile must win regardless.
    const DID_NOT_COMPILE_WITH_STALE_FAILURES: &str = "\
running 1 test
test old_crate::tests::stale_failure ... FAILED

failures:
    old_crate::tests::stale_failure

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

error[E0433]: failed to resolve: use of undeclared crate or module `frobnicate`
 --> src/new_crate.rs:3:5
  |
3 |     frobnicate::do_thing();
  |     ^^^^^^^^^^ use of undeclared crate or module `frobnicate`

error: could not compile `new_crate` (lib) due to 1 previous error
";

    const DID_NOT_COMPILE_ONLY: &str = "\
error[E0433]: failed to resolve: use of undeclared crate or module `foo`
 --> src/lib.rs:1:1
  |
1 | use foo::bar;
  | ^^^ use of undeclared crate or module `foo`

error: could not compile `mycrate` (lib) due to 1 previous error
";

    // Two crates in one invocation, one passing and one failing, with no
    // compile failure anywhere. Pins the "which crate" boundary: the failing
    // names are simply returned.
    const TWO_CRATES_ONE_PASS_ONE_FAIL: &str = "\
running 1 test
test crate_a::tests::ok_test ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

running 1 test
test crate_b::tests::broken ... FAILED

failures:

---- crate_b::tests::broken stdout ----
thread 'crate_b::tests::broken' panicked at src/lib.rs:9:9:
boom

failures:
    crate_b::tests::broken

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
";

    // "test result: FAILED" with no failures block and no headers at all --
    // no test names are parseable. This is the boundary the module exists to
    // refuse: an empty failure list here would misreport a failed run.
    const FAILED_RESULT_NO_NAMES: &str = "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s";

    // Clause 1: reads names from the indented list under "failures:".
    #[test]
    fn clause1_reads_names_from_failures_block() {
        let names = failed_tests(TWO_FAILURES_BOTH_SECTIONS);
        assert!(names.contains(&"marker::tests::precedence_violation".to_string()));
        assert!(names.contains(&"marker::tests::tab_mixed_with_spaces_is_still_a_tab".to_string()));
    }

    // Clause 2: reads names from "---- <path> stdout ----" headers too, and
    // does so even when the failures: summary block is entirely absent.
    #[test]
    fn clause2_both_sections_present_and_agreeing() {
        assert_eq!(
            failed_tests(TWO_FAILURES_BOTH_SECTIONS),
            vec![
                "marker::tests::precedence_violation".to_string(),
                "marker::tests::tab_mixed_with_spaces_is_still_a_tab".to_string(),
            ]
        );
    }

    #[test]
    fn clause2_only_headers_present() {
        assert_eq!(
            failed_tests(ONLY_HEADERS),
            vec!["xtests_0::availability::cell_3_1".to_string()]
        );
    }

    // Clause 3: union of both sources is sorted and deduplicated, including
    // across a synthetic case where the same name appears in two separate
    // "failures:" blocks (e.g. two test binaries in one invocation).
    #[test]
    fn clause3_sorted_and_deduplicated_across_repeated_blocks() {
        let output = "\
failures:
    tests::dup_case

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

failures:
    tests::dup_case

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
";
        assert_eq!(failed_tests(output), vec!["tests::dup_case".to_string()]);
    }

    #[test]
    fn clause3_name_in_both_sections_appears_once() {
        let names = failed_tests(SINGLE_FAILURE);
        assert_eq!(names, vec!["tests::single_fault".to_string()]);
    }

    // Clause 4: "test result: ok." with no failures is Passed.
    #[test]
    fn clause4_all_passed_is_passed() {
        assert_eq!(classify(ALL_PASSED), Outcome::Passed);
    }

    // Clause 5: a compile-failure marker wins even with a populated
    // failures: block from an earlier crate in the same invocation.
    #[test]
    fn clause5_compile_failure_wins_over_stale_failures_block() {
        assert_eq!(
            classify(DID_NOT_COMPILE_WITH_STALE_FAILURES),
            Outcome::DidNotCompile
        );
    }

    #[test]
    fn clause5_could_not_compile_alone_is_did_not_compile() {
        assert_eq!(classify(DID_NOT_COMPILE_ONLY), Outcome::DidNotCompile);
    }

    #[test]
    fn clause5_could_not_compile_phrase_without_error_code() {
        assert_eq!(
            classify("something failed\ncould not compile `x` (lib)\n"),
            Outcome::DidNotCompile
        );
    }

    // Clause 6: empty or unrecognisable output is Unreadable, never Passed.
    #[test]
    fn clause6_empty_output_is_unreadable() {
        assert_eq!(classify(""), Outcome::Unreadable);
        assert_eq!(failed_tests(""), Vec::<String>::new());
        assert_eq!(witness_for("arm", ""), None);
    }

    #[test]
    fn clause6_unrecognisable_output_is_unreadable_not_passed() {
        assert_eq!(classify("the dog ate the build log"), Outcome::Unreadable);
    }

    // Clause 7: witness_for is Some for Passed and Failed, None for
    // DidNotCompile and Unreadable. All four pinned.
    #[test]
    fn clause7_witness_for_passed() {
        assert_eq!(
            witness_for("arm-a", ALL_PASSED),
            Some(Witness {
                impl_arm: "arm-a".to_string(),
                failed: Vec::new(),
            })
        );
    }

    #[test]
    fn clause7_witness_for_failed() {
        assert_eq!(
            witness_for("arm-b", SINGLE_FAILURE),
            Some(Witness {
                impl_arm: "arm-b".to_string(),
                failed: vec!["tests::single_fault".to_string()],
            })
        );
    }

    #[test]
    fn clause7_witness_for_did_not_compile_is_none() {
        assert_eq!(witness_for("arm-c", DID_NOT_COMPILE_ONLY), None);
    }

    #[test]
    fn clause7_witness_for_unreadable_is_none() {
        assert_eq!(witness_for("arm-d", "garbage"), None);
    }

    // Clause 8: names are taken verbatim, including module prefixes and the
    // xtests_0 graft prefix. No stripping or normalising.
    #[test]
    fn clause8_names_taken_verbatim_with_prefixes() {
        let output = "\
failures:
    quota::tests::succeeded_resets_attempts
    xtests_0::marker::tests::tab_case

test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
";
        assert_eq!(
            failed_tests(output),
            vec![
                "quota::tests::succeeded_resets_attempts".to_string(),
                "xtests_0::marker::tests::tab_case".to_string(),
            ]
        );
    }

    // Boundary: a single failing test yields exactly one name.
    #[test]
    fn boundary_single_failing_test_yields_one_name() {
        assert_eq!(
            failed_tests(SINGLE_FAILURE),
            vec!["tests::single_fault".to_string()]
        );
        assert_eq!(
            classify(SINGLE_FAILURE),
            Outcome::Failed {
                failed: vec!["tests::single_fault".to_string()]
            }
        );
    }

    // Boundary: "test result: FAILED" but no parseable names must not become
    // an empty failure list -- Unreadable is the defensible answer.
    #[test]
    fn boundary_failed_result_with_no_parseable_names_is_unreadable_not_empty_failed() {
        assert_eq!(classify(FAILED_RESULT_NO_NAMES), Outcome::Unreadable);
        assert_ne!(
            classify(FAILED_RESULT_NO_NAMES),
            Outcome::Failed { failed: Vec::new() }
        );
    }

    // Boundary: a "failures:" block with zero entries beneath it contributes
    // no names, and the outcome then follows whatever else is present --
    // here, a "test result: ok." makes it Passed (clause 4).
    #[test]
    fn boundary_empty_failures_block_falls_through_to_passed() {
        let output = "\
running 1 test
test tests::flaky ... ok

failures:

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
";
        assert_eq!(failed_tests(output), Vec::<String>::new());
        assert_eq!(classify(output), Outcome::Passed);
    }

    // Boundary: the same empty "failures:" block with nothing else
    // recognisable falls through to Unreadable (clause 6) instead.
    #[test]
    fn boundary_empty_failures_block_with_nothing_else_is_unreadable() {
        let output = "weird tool output\nfailures:\n";
        assert_eq!(failed_tests(output), Vec::<String>::new());
        assert_eq!(classify(output), Outcome::Unreadable);
    }

    // Boundary: several crates in one invocation, one passing and one
    // failing (no compile failure) -- the failing names are returned.
    #[test]
    fn boundary_multi_crate_one_pass_one_fail_returns_failing_names() {
        assert_eq!(
            classify(TWO_CRATES_ONE_PASS_ONE_FAIL),
            Outcome::Failed {
                failed: vec!["crate_b::tests::broken".to_string()]
            }
        );
    }

    // Superset status: an unenumerated compile-error code is still
    // recognised, because the marker is matched structurally rather than
    // against a fixed list of codes.
    #[test]
    fn structural_error_code_match_is_not_a_fixed_list() {
        assert_eq!(
            classify("error[E9999]: some future rustc diagnostic\n"),
            Outcome::DidNotCompile
        );
    }

    // A line mentioning "error[E..." without the closing "]:" is not a
    // compile-failure marker -- it must not cause a false DidNotCompile.
    #[test]
    fn near_miss_error_code_syntax_is_not_a_compile_marker() {
        assert_eq!(
            classify("this test prints error[E0433 without a close bracket\n"),
            Outcome::Unreadable
        );
    }
}

// ESCALATED: 1 confirmed finding(s) by critic claude-sonnet, found on codex-luna.
// Promoted from an executed proof that passed the reference veto. Provenance is
// recorded so a bad test can be traced and retired.  (bead farmerbob-mqr)
#[cfg(test)]
mod escalated_testout_codex_luna {
    use super::*;

    #[test]
    // claim_1: `has_compile_diagnostic` treats "error[E" and "]:" as independent, non-adjacent
    // substring checks on a line, so a failing test's own captured assertion text that happens to
    // contain both (with no real rustc diagnostic anywhere in the output) is misclassified as a
    // compile failure, discarding genuine failure evidence.
    fn claim_1() {
        let output = r#"running 1 test
test tests::odd_message ... FAILED

failures:

---- tests::odd_message stdout ----
thread 'tests::odd_message' panicked at src/lib.rs:10:5:
assertion failed: msg == "error[E1] unexpected input, got ]: token"
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

failures:
    tests::odd_message

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
"#;

        assert_eq!(
            classify(output),
            Outcome::Failed {
                failed: vec!["tests::odd_message".to_string()],
            }
        );
    }
}
