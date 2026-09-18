//! Whether a filtered `cargo test` run checked anything at all.
//!
//! `fb-escalate.sh verify <task>` runs the escalated conformance suite by
//! filtering `cargo test` to the module name. On a real escalated file it
//! printed
//!
//! ```text
//! test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1440 filtered out
//! ```
//!
//! and reported `ok`. Zero tests ran. The file existed and contained a real
//! module with a real test; the filter matched none of it. The instrument
//! could not find what it was asked to check and said everything was fine.
//! Reading `0 passed` as a pass is this project's recurring bug, and it sat
//! inside the anti-invalid-test check -- the thing whose entire job is to
//! stop a bad test being trusted. This module gives that state a variant of
//! its own so it can never again be mistaken for a pass.
//!
//! Two modules already read `cargo test` output for other questions:
//! [`crate::cell_record::read_cell`] asks whether the build itself failed,
//! and [`crate::testout::failed_tests`] asks which tests failed. This module
//! answers a third: did anything run at all. The questions do not share a
//! parser because they do not share an answer -- a run can be fully readable
//! here as [`Verified::MatchedNothing`] while telling [`crate::testout`]
//! nothing worth reporting, because the counts this module reads are not the
//! names that one needs.
//!
//! No I/O: the caller captures the run's combined stdout+stderr and hands
//! over the text, exactly as for [`crate::testout`] and
//! [`crate::cell_record`].

/// What a filtered `cargo test` run actually established.
///
/// A CLOSED set of four. The runner's OUTPUT FORMAT is an OPEN subset:
/// cargo's text is not a stability guarantee, so a shape this module does
/// not recognise falls to [`Verified::Unreadable`] and never to
/// [`Verified::Passed`]. Unreadable is the safe direction: it means
/// "nothing was established", which [`was_checked`] refuses to treat as a
/// check, where a false [`Verified::Passed`] would mean "everything fine"
/// from an instrument that never found its subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verified {
    /// Tests matched the filter and all of them passed.
    Passed {
        /// How many ran. Always at least one BY CONSTRUCTION: a run in
        /// which zero executed is [`Verified::MatchedNothing`], because a
        /// run that established nothing is not a pass. No code path
        /// produces `Passed` with `ran: 0`, and none needs to guard
        /// against it.
        ran: u32,
    },
    /// Tests matched and some failed.
    Failed {
        /// How many ran: passed plus failed, summed here because the
        /// runner never prints a total. Always at least `failed`, being
        /// that plus the passes. Saturated at [`u32::MAX`] rather than
        /// allowed to wrap -- a wrapped total would read as a small,
        /// ordinary run.
        ran: u32,
        /// How many failed.
        failed: u32,
    },
    /// The filter matched NOTHING. Not a pass: the instrument could not
    /// find what it was asked to check.
    ///
    /// This is the variant for `0 passed; 0 failed` in EVERY case -- with
    /// a non-zero `filtered out` count, with `filtered out: 0`, and with
    /// the clause absent altogether. A suite where nothing exists and
    /// nothing ran established nothing either, so `0 filtered out` is
    /// reported as [`Some`]`(0)`, not read as a pass; there is no
    /// `Passed { ran: 0 }` in this enum.
    MatchedNothing {
        /// How many tests existed but were filtered out, when the runner
        /// says. [`None`] when the clause is absent or its count does not
        /// parse as a number: an absent count is not zero, and an unread
        /// one is not evidence either. Both spellings mean the same thing
        /// here because the variant is decided by the zero tests that ran,
        /// not by the count of the ones that did not.
        filtered_out: Option<u32>,
    },
    /// The output could not be read as a test result at all: empty
    /// output, no `test result:` line (a run that did not build), or a
    /// status word or count shape this module does not recognise. Never a
    /// pass -- see the open-subset note on this enum.
    Unreadable,
}

/// Read a filtered `cargo test` run's output.
///
/// Decision order, fixed here so it need not be rediscovered:
///
/// 1. The LAST line beginning `test result:` is the one read. A
///    multi-crate invocation prints one result line per crate, and the
///    filter applies to the final crate under test, so an earlier crate's
///    totals are not this run's answer. When that last line cannot be
///    read, the answer is [`Verified::Unreadable`] -- never an earlier
///    crate's result, which would attribute one crate's totals to a
///    different crate: a false answer wearing the right shape.
/// 2. That line must begin with the status word `ok.` or `FAILED.` and
///    must carry the two counts every result line prints -- `N passed;`
///    `M failed;` -- as numbers. Any other status word, or missing or
///    non-numeric counts there, is a shape this module does not recognise
///    and yields [`Verified::Unreadable`]. The counts decide the variant;
///    the word only announces that a result line is present.
/// 3. `0 passed; 0 failed` is [`Verified::MatchedNothing`], in every case
///    -- see the variant's documentation. A `filtered out` count that does
///    not parse does not poison the line: the result line was read, only
///    that count was not, so it reports [`None`].
/// 4. Otherwise `failed == 0` is [`Verified::Passed`] and `failed > 0` is
///    [`Verified::Failed`], with `ran` = passed + failed.
///
/// Every count saturates at [`u32::MAX`]: a number that large is already
/// nonsense, and the variant it lands in is what matters, not the exact
/// figure -- silently wrapping would turn a huge count into a small one
/// and could change the variant. Count segments with other labels
/// (`ignored`, `measured`, whatever a future cargo adds) are not this
/// module's question and are skipped, so their presence never blocks a
/// readable line.
pub fn read(output: &str) -> Verified {
    match last_result_line(output) {
        Some(line) => read_result_line(line),
        None => Verified::Unreadable,
    }
}

/// Whether this result may be treated as the suite having been checked.
///
/// True ONLY for [`Verified::Passed`] and [`Verified::Failed`] -- both
/// mean tests ran. [`Verified::MatchedNothing`] and
/// [`Verified::Unreadable`] mean nothing was established, and a caller
/// that trusts either has rebuilt the bug that produced this module:
/// reporting `ok` on a run that never found its subject.
pub fn was_checked(v: &Verified) -> bool {
    matches!(v, Verified::Passed { .. } | Verified::Failed { .. })
}

/// The marker that opens every libtest summary line.
const RESULT_PREFIX: &str = "test result:";

/// The two status words a result line may begin with. Both are pinned to
/// cargo's exact spellings; anything else is an unrecognised shape.
const OK_STATUS: &str = "ok.";
const FAILED_STATUS: &str = "FAILED.";

/// The only count labels this module reads. Everything else on the line
/// (`ignored`, `measured`, `finished in ...`) answers a question someone
/// else is asking.
const PASSED_LABEL: &str = "passed";
const FAILED_LABEL: &str = "failed";
const FILTERED_OUT_LABEL: &str = "filtered out";

/// The last line of `output` beginning with [`RESULT_PREFIX`], trimmed.
/// The last, not the first: see step 1 of [`read`].
fn last_result_line(output: &str) -> Option<&str> {
    output
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with(RESULT_PREFIX))
        .map(str::trim)
}

/// Classify one result line -- already trimmed by [`last_result_line`].
fn read_result_line(line: &str) -> Verified {
    let rest = match line.strip_prefix(RESULT_PREFIX) {
        Some(rest) => rest.trim_start(),
        None => return Verified::Unreadable,
    };
    let counts_text = if let Some(rest) = rest.strip_prefix(OK_STATUS) {
        rest
    } else if let Some(rest) = rest.strip_prefix(FAILED_STATUS) {
        rest
    } else {
        // A status word cargo does not print: not a result line this
        // module can read, and never a pass.
        return Verified::Unreadable;
    };

    let mut passed = None;
    let mut failed = None;
    let mut filtered_out = None;
    for segment in counts_text.split(';') {
        let (value, label) = split_count(segment);
        match label {
            PASSED_LABEL => passed = value,
            FAILED_LABEL => failed = value,
            FILTERED_OUT_LABEL => filtered_out = value,
            _ => {} // ignored, measured, future additions: not our question
        }
    }

    let (Some(passed), Some(failed)) = (passed, failed) else {
        // The two counts every result line carries are missing or not
        // numbers. The line announced itself as a result but cannot say
        // what happened: unrecognised shape, nothing established.
        return Verified::Unreadable;
    };

    match (passed, failed) {
        (0, 0) => Verified::MatchedNothing { filtered_out },
        (ran, 0) => Verified::Passed { ran },
        (ran, count_failed) => Verified::Failed {
            ran: ran.saturating_add(count_failed),
            failed: count_failed,
        },
    }
}

/// Split one `;`-separated count segment into its number, if it carries
/// one, and its label: `"3 passed"` is `(Some(3), "passed")`, and
/// `"filtered out"` with the number missing is `(None, "filtered out")`.
/// The label has whitespace trimmed; the number is read digit by digit
/// and saturates at [`u32::MAX`] -- see [`saturating_u32`].
fn split_count(segment: &str) -> (Option<u32>, &str) {
    let segment = segment.trim();
    let digits_end = segment
        .bytes()
        .position(|byte| !byte.is_ascii_digit())
        .unwrap_or(segment.len());
    let (digits, label) = segment.split_at(digits_end);
    let value = if digits.is_empty() {
        None
    } else {
        Some(saturating_u32(digits))
    };
    (value, label.trim())
}

/// Parse a slice of ASCII digits, saturating at [`u32::MAX`]. Pinned here
/// rather than delegated to a fallible parse: a count too big for the
/// type is already nonsense, and wrapping it into a small number could
/// change which variant the run lands in. The input is all ASCII digits
/// by construction -- [`split_count`] cuts it at the first byte that is
/// not one -- so no digit subtraction can go out of range.
fn saturating_u32(digits: &str) -> u32 {
    let mut acc: u64 = 0;
    for digit in digits.bytes() {
        acc = acc
            .saturating_mul(10)
            .saturating_add(u64::from(digit - b'0'));
    }
    u32::try_from(acc).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_PASSED: &str = "\
running 3 tests
test suite_match::counts_the_passes ... ok
test suite_match::counts_the_failures ... ok
test suite_match::last_line_wins ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
";

    const ONE_FAILURE: &str = "\
running 3 tests
test suite_match::counts_the_passes ... ok
test suite_match::counts_the_failures ... ok
test suite_match::rejects_a_zero_run ... FAILED

failures:

---- suite_match::rejects_a_zero_run stdout ----
thread 'suite_match::rejects_a_zero_run' panicked at src/suite_match.rs:1:1:
0 passed; 0 failed is not a pass

test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
";

    // The line this module exists for, exactly as fb-escalate.sh printed
    // it: no `finished in` suffix, `ok.`, and nothing ran.
    const THE_INCIDENT: &str =
        "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1440 filtered out";

    // The incident in its habitat: a workspace invocation where an earlier
    // crate ran its tests and the final crate -- the one the escalated
    // filter named -- matched everything out. The first result line must
    // not be the answer.
    const MULTI_CRATE_MATCHED_NOTHING: &str = "\
running 12 tests
test escalation::proves::the_proof ... ok
test escalation::proves::the_veto ... ok

test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1440 filtered out
";

    // Same shape, other direction: the earlier crate matched nothing and
    // the final crate is the one that ran.
    const MULTI_CRATE_FINAL_CRATE_RAN: &str = "\
running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out

running 2 tests
test suite_match::final_crate_wins ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
";

    // Clause 1, in full output: counts become the run count.
    #[test]
    fn clause1_ok_with_counts_is_passed() {
        assert_eq!(read(ALL_PASSED), Verified::Passed { ran: 3 });
    }

    // Clause 1 on the bare line, with no `running` header and no `finished
    // in` suffix -- the suffix is not part of the contract (the incident
    // line has none).
    #[test]
    fn clause1_bare_result_line_is_enough() {
        let line = "test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out";
        assert_eq!(read(line), Verified::Passed { ran: 3 });
    }

    // Clause 2: ran is passed plus failed -- the runner never prints a
    // total, so the reader sums it.
    #[test]
    fn clause2_failed_sums_ran_from_passed_and_failed() {
        assert_eq!(read(ONE_FAILURE), Verified::Failed { ran: 3, failed: 1 });
    }

    // Clause 3: THE clause. Zero tests that matched is not a pass, even
    // though the runner says `ok.` -- and 1440 is the count of tests that
    // existed but were never run.
    #[test]
    fn clause3_zero_matched_is_matched_nothing_not_passed() {
        assert_eq!(
            read(THE_INCIDENT),
            Verified::MatchedNothing {
                filtered_out: Some(1440)
            }
        );
        assert_ne!(read(THE_INCIDENT), Verified::Passed { ran: 0 });
        assert!(!was_checked(&read(THE_INCIDENT)));
    }

    // Clause 4: the same zero-run with the `filtered out` clause absent
    // entirely. An absent count is not zero -- None, not Some(0).
    #[test]
    fn clause4_absent_filtered_out_is_none_not_zero() {
        let line = "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured";
        assert_eq!(read(line), Verified::MatchedNothing { filtered_out: None });
    }

    // Clause 5, both spellings: with and without the clause, a zero run is
    // MatchedNothing. Pinned together because they are the pair a reader
    // expects to differ.
    #[test]
    fn clause5_zero_run_is_matched_nothing_in_every_case() {
        let without = "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured";
        let with = "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out";
        // The clause's presence changes the reported count (clause 4) but
        // never the variant: both are MatchedNothing, neither a pass.
        assert!(matches!(read(without), Verified::MatchedNothing { .. }));
        assert!(matches!(read(with), Verified::MatchedNothing { .. }));
    }

    // Boundary: `0 filtered out` is Some(0), not None -- and not a pass. A
    // suite where nothing exists and nothing ran established nothing too.
    #[test]
    fn boundary_zero_filtered_out_is_some_zero() {
        let line = "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out";
        assert_eq!(
            read(line),
            Verified::MatchedNothing {
                filtered_out: Some(0)
            }
        );
    }

    // Boundary at one: the smallest passing run, and the smallest failing
    // one, each on their own line.
    #[test]
    fn boundary_one_is_the_smallest_real_run() {
        let passed = "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out";
        let failed =
            "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out";
        assert_eq!(read(passed), Verified::Passed { ran: 1 });
        assert_eq!(read(failed), Verified::Failed { ran: 1, failed: 1 });
    }

    // Clause 6: was_checked is the gate. True for both ran variants, false
    // for both established-nothing variants, all four pinned.
    #[test]
    fn clause6_was_checked_true_only_when_tests_ran() {
        assert!(was_checked(&Verified::Passed { ran: 1 }));
        assert!(was_checked(&Verified::Failed { ran: 2, failed: 1 }));
        assert!(!was_checked(&Verified::MatchedNothing {
            filtered_out: Some(1440)
        }));
        assert!(!was_checked(&Verified::MatchedNothing {
            filtered_out: None
        }));
        assert!(!was_checked(&Verified::Unreadable));
    }

    // Clause 7: empty output is Unreadable -- never `Passed`, and there is
    // no `Passed { ran: 0 }` to fall into.
    #[test]
    fn clause7_empty_output_is_unreadable() {
        assert_eq!(read(""), Verified::Unreadable);
        assert_ne!(read(""), Verified::Passed { ran: 0 });
    }

    // Clause 7's neighbourhood: output with no `test result:` line at all
    // -- a run that never built, or output from some other tool -- cannot
    // be read as a test result either.
    #[test]
    fn clause7_no_result_line_is_unreadable() {
        let did_not_compile = "\
error[E0433]: failed to resolve: use of undeclared crate or module `frobnicate`
error: could not compile `escalated` (lib) due to 1 previous error
";
        assert_eq!(read(did_not_compile), Verified::Unreadable);
        assert_eq!(read("the dog ate the build log"), Verified::Unreadable);
    }

    // Clause 8: several `test result:` lines are read from the LAST one --
    // the filter applies to the final crate under test -- in both
    // directions.
    #[test]
    fn clause8_last_result_line_wins() {
        assert_eq!(
            read(MULTI_CRATE_MATCHED_NOTHING),
            Verified::MatchedNothing {
                filtered_out: Some(1440)
            }
        );
        assert_eq!(
            read(MULTI_CRATE_FINAL_CRATE_RAN),
            Verified::Passed { ran: 2 }
        );
    }

    // Superset doctrine: a status word cargo does not print is an
    // unrecognised shape -- Unreadable, never Passed, however plausible
    // the counts after it look.
    #[test]
    fn unrecognised_status_word_is_unreadable_never_passed() {
        let line = "test result: BANANA. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out";
        assert_eq!(read(line), Verified::Unreadable);
    }

    // Clause 5 meets an `ignored` count: tests that were skipped did not
    // run either. Zero ran is zero ran.
    #[test]
    fn boundary_ignored_tests_did_not_run() {
        let line = "test result: ok. 0 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out";
        assert_eq!(
            read(line),
            Verified::MatchedNothing {
                filtered_out: Some(0)
            }
        );
    }

    // Boundary: a `filtered out` count that does not parse leaves the line
    // readable -- the result line was read, only that count was not --
    // whether the run matched nothing or matched something.
    #[test]
    fn boundary_unparseable_filtered_out_is_none_not_unreadable() {
        let zero = "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; many filtered out";
        assert_eq!(read(zero), Verified::MatchedNothing { filtered_out: None });
        let some = "test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; many filtered out";
        assert_eq!(read(some), Verified::Passed { ran: 3 });
    }

    // Boundary: counts larger than u32::MAX saturate at u32::MAX. Pinned
    // per variant, because wrapping any of them into a small number could
    // change the variant the run lands in.
    #[test]
    fn boundary_huge_counts_saturate_not_wrap() {
        let huge = u32::MAX.to_string();
        let big = "99999999999";
        let passed = format!(
            "test result: ok. {big} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"
        );
        assert_eq!(read(&passed), Verified::Passed { ran: u32::MAX });

        let failed = format!(
            "test result: FAILED. {big} passed; {big} failed; 0 ignored; 0 measured; 0 filtered out"
        );
        assert_eq!(
            read(&failed),
            Verified::Failed {
                ran: u32::MAX,
                failed: u32::MAX
            }
        );

        let filtered = format!(
            "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; {huge}0 filtered out"
        );
        assert_eq!(
            read(&filtered),
            Verified::MatchedNothing {
                filtered_out: Some(u32::MAX)
            }
        );
    }

    // Boundary: the sum saturates too. passed + failed above u32::MAX must
    // not wrap into a small, ordinary-looking total.
    #[test]
    fn boundary_ran_sum_saturates_not_wraps() {
        let line = "test result: FAILED. 4294967295 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out";
        assert_eq!(
            read(line),
            Verified::Failed {
                ran: u32::MAX,
                failed: 2
            }
        );
    }
}
