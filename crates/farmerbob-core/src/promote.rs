//! Promotion of falsifiable critique claims into executable tests.
//!
//! A critic may hold opinions about a rival implementation, but opinions do
//! not score. Objective assertions, in contrast, can be checked by the
//! harness, so they are lifted out of a critique and promoted into tests.
//! This module holds the pure decision logic: it performs no I/O and never
//! executes a test. It only decides what *would* be run.
//!
//! The pipeline scans each assertion once, in order: blank required fields
//! are refused first, then claims that allege nothing, then repeats of an
//! already promoted claim. Whatever survives is kept with a stable
//! [`fingerprint`] identity. [`objective_leaks`] answers the mirror question:
//! whether a judgement smuggles in something the harness could check.

/// A line a critic wrote about a rival implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Assertion {
    /// Falsifiable: names a target, an input, what the critic expected, what they say happens.
    Claim {
        /// Which implementation the claim is about.
        target: String,
        /// The input the claim pins down.
        input: String,
        /// What the critic says should happen for that input.
        expect: String,
        /// What the critic says actually happens instead.
        actual: String,
    },
    /// Opinion. Recorded, never scored, never executed.
    Judgement {
        /// Which implementation the opinion is about.
        target: String,
        /// The opinionated text. Never executed; inspected by [`objective_leaks`].
        text: String,
    },
}

/// Why an assertion could not become a test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// `expect` and `actual` are the same: nothing is being alleged.
    NotFalsifiable,
    /// A required field is empty.
    Incomplete {
        /// Name of the first blank required field, in declaration order.
        field: String,
    },
    /// A judgement was submitted where a claim was required.
    NotAClaim,
    /// The same claim, already promoted.
    Duplicate,
    /// The claim's `actual` quotes text that appears in neither source.
    Fabricated {
        /// The first quoted fragment that could not be found, verbatim.
        quote: String,
    },
    /// The claim's `actual` quotes text found in the critic's source but not the subject.
    Projected {
        /// The first quoted fragment found in the critic and not the subject.
        quote: String,
    },
}

/// A claim that earned execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotedTest {
    /// Which implementation the test targets, as the critic wrote it.
    pub target: String,
    /// The input under test, as the critic wrote it.
    pub input: String,
    /// The expected outcome, as the critic wrote it.
    pub expect: String,
    /// Stable identity: equal for two claims alleging the same thing, so the same
    /// assertion made by two critics is executed once.
    pub fingerprint: String,
}

/// Shared normalisation: surrounding whitespace folded away, letter case folded away.
fn normalise(value: &str) -> String {
    value.trim().to_lowercase()
}

/// Name of the first blank required field of a claim, if any.
///
/// Fields are inspected in declaration order (`target`, `input`, `expect`,
/// `actual`); a field counts as blank when it is empty or whitespace-only.
fn first_blank_field(target: &str, input: &str, expect: &str, actual: &str) -> Option<String> {
    if target.trim().is_empty() {
        Some(String::from("target"))
    } else if input.trim().is_empty() {
        Some(String::from("input"))
    } else if expect.trim().is_empty() {
        Some(String::from("expect"))
    } else if actual.trim().is_empty() {
        Some(String::from("actual"))
    } else {
        None
    }
}

/// Promote what can be executed; reject the rest with a reason.
///
/// Order is preserved, and a rejection never aborts the batch: one unfalsifiable line
/// must not discard the critique it arrived in.
///
/// A blank required field is reported before falsifiability is considered, and
/// when several fields are blank the first in declaration order (`target`,
/// `input`, `expect`, `actual`) is named. Repeats are detected with
/// [`fingerprint`]: the first occurrence is kept and later ones are rejected as
/// [`Rejection::Duplicate`]. Judgements are never malformed, only misplaced,
/// so they are always rejected as [`Rejection::NotAClaim`].
pub fn promote(assertions: &[Assertion]) -> (Vec<PromotedTest>, Vec<(usize, Rejection)>) {
    let mut promoted: Vec<PromotedTest> = Vec::new();
    let mut rejections: Vec<(usize, Rejection)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for (index, assertion) in assertions.iter().enumerate() {
        match assertion {
            Assertion::Judgement { .. } => {
                rejections.push((index, Rejection::NotAClaim));
            }
            Assertion::Claim {
                target,
                input,
                expect,
                actual,
            } => {
                if let Some(field) = first_blank_field(target, input, expect, actual) {
                    rejections.push((index, Rejection::Incomplete { field }));
                    continue;
                }
                if normalise(expect) == normalise(actual) {
                    rejections.push((index, Rejection::NotFalsifiable));
                    continue;
                }
                let identity = fingerprint(target, input, expect);
                if seen.contains(&identity) {
                    rejections.push((index, Rejection::Duplicate));
                    continue;
                }
                seen.push(identity.clone());
                promoted.push(PromotedTest {
                    target: target.clone(),
                    input: input.clone(),
                    expect: expect.clone(),
                    fingerprint: identity,
                });
            }
        }
    }
    (promoted, rejections)
}

/// Check a claim's quoted evidence against the subject and critic sources.
///
/// Returns `None` when the assertion is a judgement, when it has no usable quoted
/// fragment, or when every usable fragment occurs in `subject`. An empty `subject`
/// means that no source was supplied, so even a quoted claim returns `None`, never
/// [`Rejection::Fabricated`]; a missing haystack is not evidence that a needle is
/// absent. The set of ways a claim can be wrong is open, and this function decides
/// only fabrication and projection. Surviving this check has not shown a claim true;
/// false absence (alleging that something is missing when the file contains it) is
/// another known shape this function does not detect.
///
/// Fragments are runs between paired `"` characters. Empty and whitespace-only
/// fragments are ignored, and an unterminated final quote is ignored. Nested
/// quoting is not supported: a fragment cannot contain `"` by construction, which
/// is sufficient for the claim format. Matching is exact and byte-for-byte by
/// design; guessing at equivalent whitespace or spelling would silently permit
/// fabrication.
pub fn verify_quotes(a: &Assertion, subject: &str, critic: &str) -> Option<Rejection> {
    let Assertion::Claim { actual, .. } = a else {
        return None;
    };
    if subject.is_empty() {
        return None;
    }

    let mut first_fabricated: Option<String> = None;
    let mut first_projected: Option<String> = None;
    let mut quote_start: Option<usize> = None;
    for (index, byte) in actual.bytes().enumerate() {
        if byte != b'"' {
            continue;
        }
        if let Some(start) = quote_start.take() {
            let quote = &actual[start..index];
            if quote.trim().is_empty() {
                continue;
            }
            if subject.contains(quote) {
                continue;
            }
            if critic.contains(quote) {
                if first_projected.is_none() {
                    first_projected = Some(String::from(quote));
                }
            } else if first_fabricated.is_none() {
                first_fabricated = Some(String::from(quote));
            }
        } else {
            quote_start = Some(index + 1);
        }
    }

    first_projected
        .map(|quote| Rejection::Projected { quote })
        .or_else(|| first_fabricated.map(|quote| Rejection::Fabricated { quote }))
}

/// Verify many assertions against one subject and one critic, in input order.
///
/// Returns `(index, rejection)` for each assertion refused, in ascending index
/// order. An empty input returns an empty vector, meaning that nothing was
/// examined; an empty result for a non-empty input means that nothing was refused.
/// The caller is responsible for knowing which of those two facts it asked for.
pub fn verify_all(
    assertions: &[Assertion],
    subject: &str,
    critic: &str,
) -> Vec<(usize, Rejection)> {
    assertions
        .iter()
        .enumerate()
        .filter_map(|(index, assertion)| {
            verify_quotes(assertion, subject, critic).map(|rejection| (index, rejection))
        })
        .collect()
}

/// Normalised identity of a claim, ignoring surrounding whitespace and letter case.
///
/// Case and surrounding whitespace are folded away, and `actual` plays no
/// part, so two wordings of the same alleged failure share one identity. The
/// layout of the returned string is unspecified; only its equality behaviour
/// is meaningful.
pub fn fingerprint(target: &str, input: &str, expect: &str) -> String {
    let target = normalise(target);
    let input = normalise(input);
    let expect = normalise(expect);
    format!(
        "{}:{}|{}:{}|{}:{}",
        target.len(),
        target,
        input.len(),
        input,
        expect.len(),
        expect
    )
}

/// True when the lowercased text holds a bare integer directly followed by `tests`.
///
/// "Bare" means the digits stand alone: the character before them (if any) is
/// not alphanumeric, and `tests` is followed by a non-alphanumeric character
/// or the end of the text. All indexing is bounds-checked.
fn has_counted_tests(lower: &str) -> bool {
    let bytes = lower.as_bytes();
    let word = b"tests";
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let bare_before = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            let mut k = j;
            while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                k += 1;
            }
            if bare_before
                && k > j
                && k + word.len() <= bytes.len()
                && bytes[k..k + word.len()] == *word
                && (k + word.len() == bytes.len() || !bytes[k + word.len()].is_ascii_alphanumeric())
            {
                return true;
            }
            i = j;
        } else {
            i += 1;
        }
    }
    false
}

/// True when a judgement text asserts something the harness could check.
///
/// Matching is word-level and case-insensitive over the verb families behind
/// `passes`, `fails`, `panics`, `compiles` and `returns`, plus the counted
/// pattern handled by [`has_counted_tests`]. Words that merely embed a
/// keyword (such as "surpasses") do not count.
fn is_objective(text: &str) -> bool {
    let lower = text.to_lowercase();
    if has_counted_tests(&lower) {
        return true;
    }
    for word in lower.split(|c: char| !c.is_alphanumeric()) {
        if matches!(
            word,
            "pass"
                | "passes"
                | "passed"
                | "passing"
                | "fail"
                | "fails"
                | "failed"
                | "failing"
                | "panic"
                | "panics"
                | "panicked"
                | "panicking"
                | "compile"
                | "compiles"
                | "compiled"
                | "compiling"
                | "return"
                | "returns"
                | "returned"
                | "returning"
        ) {
            return true;
        }
    }
    false
}

/// Does this critique contain an objective assertion where only judgement was allowed?
///
/// The handoff and the critique are both forbidden from stating anything the harness can
/// check. Returns the indices of the offending entries.
///
/// Only [`Assertion::Judgement`] entries are inspected; claims are never
/// leaks. A judgement leaks when any word is an inflection of `pass`, `fail`,
/// `panic`, `compile` or `return`, or when a bare integer is directly
/// followed by `tests`.
pub fn objective_leaks(assertions: &[Assertion]) -> Vec<usize> {
    let mut leaks: Vec<usize> = Vec::new();
    for (index, assertion) in assertions.iter().enumerate() {
        let leaked = match assertion {
            Assertion::Judgement { text, .. } => is_objective(text),
            Assertion::Claim { .. } => false,
        };
        if leaked {
            leaks.push(index);
        }
    }
    leaks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(target: &str, input: &str, expect: &str, actual: &str) -> Assertion {
        Assertion::Claim {
            target: String::from(target),
            input: String::from(input),
            expect: String::from(expect),
            actual: String::from(actual),
        }
    }

    fn judgement(target: &str, text: &str) -> Assertion {
        Assertion::Judgement {
            target: String::from(target),
            text: String::from(text),
        }
    }

    #[test]
    fn expect_equal_actual_is_not_falsifiable() {
        let assertions = [claim("sort", "[]", "[]", "[]")];
        let (promoted, rejections) = promote(&assertions);
        assert!(promoted.is_empty());
        assert_eq!(rejections, [(0, Rejection::NotFalsifiable)]);
    }

    #[test]
    fn case_and_whitespace_differences_still_count_as_equal() {
        let assertions = [claim("sort", "[3, 1]", "  [1, 3] ", "[1, 3]")];
        let (promoted, rejections) = promote(&assertions);
        assert!(promoted.is_empty());
        assert_eq!(rejections, [(0, Rejection::NotFalsifiable)]);

        let assertions = [claim("sort", "[3, 1]", "Sorted", "sOrTeD")];
        let (promoted, rejections) = promote(&assertions);
        assert!(promoted.is_empty());
        assert_eq!(rejections, [(0, Rejection::NotFalsifiable)]);
    }

    #[test]
    fn empty_input_field_names_that_field() {
        let assertions = [claim("sort", "", "[1]", "[2]")];
        let (_, rejections) = promote(&assertions);
        assert_eq!(
            rejections,
            [(
                0,
                Rejection::Incomplete {
                    field: String::from("input")
                }
            )]
        );
    }

    #[test]
    fn whitespace_only_fields_are_incomplete() {
        let cases = [
            (claim("   ", "x", "a", "b"), "target"),
            (claim("t", "\t ", "a", "b"), "input"),
            (claim("t", "x", "  ", "b"), "expect"),
            (claim("t", "x", "a", "\n"), "actual"),
        ];
        for (assertion, field) in cases {
            let (promoted, rejections) = promote(&[assertion]);
            assert!(promoted.is_empty());
            assert_eq!(
                rejections,
                [(
                    0,
                    Rejection::Incomplete {
                        field: String::from(field)
                    }
                )]
            );
        }
    }

    #[test]
    fn every_blank_field_is_named_when_it_is_the_only_blank() {
        for field in ["target", "input", "expect", "actual"] {
            let (target, input, expect, actual) = match field {
                "target" => ("", "x", "a", "b"),
                "input" => ("t", "", "a", "b"),
                "expect" => ("t", "x", "", "b"),
                _ => ("t", "x", "a", ""),
            };
            let (_, rejections) = promote(&[claim(target, input, expect, actual)]);
            assert_eq!(
                rejections,
                [(
                    0,
                    Rejection::Incomplete {
                        field: String::from(field)
                    }
                )]
            );
        }
    }

    #[test]
    fn blank_fields_beat_falsifiability() {
        // Both sides blank: there is nothing to compare, so the refusal names
        // the missing field rather than alleging sameness.
        let assertions = [claim("t", "x", "", "")];
        let (_, rejections) = promote(&assertions);
        assert_eq!(
            rejections,
            [(
                0,
                Rejection::Incomplete {
                    field: String::from("expect")
                }
            )]
        );
    }

    #[test]
    fn two_critics_alleging_the_same_thing_yield_one_test() {
        let assertions = [
            claim("sort", "[]", "[]", "panics"),
            claim("sort", "[]", "[]", "hangs forever"),
        ];
        let (promoted, rejections) = promote(&assertions);
        assert_eq!(promoted.len(), 1);
        assert_eq!(rejections, [(1, Rejection::Duplicate)]);
    }

    #[test]
    fn first_duplicate_is_kept() {
        let assertions = [
            claim("sort", "[]", "[]", "panics"),
            claim("sort", "[]", "[]", "hangs"),
        ];
        let (promoted, _) = promote(&assertions);
        assert_eq!(promoted.len(), 1);
        assert_eq!(promoted[0].target, "sort");
        assert_eq!(promoted[0].input, "[]");
        assert_eq!(promoted[0].expect, "[]");
    }

    #[test]
    fn duplicates_match_across_case_and_whitespace() {
        let assertions = [
            claim("Sort", "  [] ", " []", "panics"),
            claim("  sort ", "[]", "[] ", "hangs"),
        ];
        let (promoted, rejections) = promote(&assertions);
        assert_eq!(promoted.len(), 1);
        assert_eq!(rejections, [(1, Rejection::Duplicate)]);
    }

    #[test]
    fn different_expectations_are_different_tests() {
        let assertions = [
            claim("sort", "[]", "[]", "panics"),
            claim("sort", "[]", "[0]", "panics"),
        ];
        let (promoted, rejections) = promote(&assertions);
        assert_eq!(promoted.len(), 2);
        assert!(rejections.is_empty());
    }

    #[test]
    fn judgement_is_not_a_claim() {
        let assertions = [judgement("sort", "this allocates on every call")];
        let (promoted, rejections) = promote(&assertions);
        assert!(promoted.is_empty());
        assert_eq!(rejections, [(0, Rejection::NotAClaim)]);
    }

    #[test]
    fn judgement_is_never_malformed() {
        // Even a judgement with blank fields is simply not a test.
        let assertions = [judgement("", "   ")];
        let (promoted, rejections) = promote(&assertions);
        assert!(promoted.is_empty());
        assert_eq!(rejections, [(0, Rejection::NotAClaim)]);
    }

    #[test]
    fn objective_leaks_catches_all_tests_pass_and_ignores_fragile() {
        let assertions = [
            judgement("sort", "all tests pass"),
            judgement("sort", "this seems fragile"),
        ];
        assert_eq!(objective_leaks(&assertions), [0]);
    }

    #[test]
    fn objective_leaks_flags_each_keyword_family() {
        let texts = [
            "it passes on retry",
            "it fails on empty input",
            "it panics on zero",
            "it compiles without warnings",
            "it returns zero",
            "ALL TESTS PASSED yesterday",
        ];
        for text in texts {
            let assertions = [judgement("sort", text)];
            assert_eq!(objective_leaks(&assertions), [0], "missed: {text}");
        }
    }

    #[test]
    fn objective_leaks_flags_bare_integer_followed_by_tests() {
        for text in [
            "breaks 5 tests",
            "fixed 12  tests",
            "1 test",
            "3 TESTS fail",
        ] {
            let assertions = [judgement("sort", text)];
            let leaked = if text == "1 test" {
                Vec::<usize>::new()
            } else {
                vec![0]
            };
            assert_eq!(objective_leaks(&assertions), leaked, "text: {text}");
        }
    }

    #[test]
    fn objective_leaks_ignores_claims_and_subjective_text() {
        let assertions = [
            claim("sort", "[]", "[]", "it fails loudly"),
            judgement("sort", "this seems fragile"),
            judgement(
                "sort",
                "this allocates on every call, which will hurt under load",
            ),
            judgement("sort", "the design surpasses expectations"),
        ];
        assert!(objective_leaks(&assertions).is_empty());
    }

    #[test]
    fn objective_leaks_reports_every_offending_index() {
        let assertions = [
            judgement("a", "seems fragile"),
            judgement("b", "it fails sometimes"),
            claim("c", "x", "a", "b"),
            judgement("d", "breaks 5 tests"),
        ];
        assert_eq!(objective_leaks(&assertions), [1, 3]);
    }

    #[test]
    fn one_bad_line_does_not_discard_the_batch() {
        let assertions = [
            claim("sort", "[2, 1]", "[1, 2]", "panics"),
            claim("sort", "[2, 1]", "[1, 2]", "[1, 2]"),
            judgement("sort", "seems slow"),
            claim("sort", "[0]", "[0]", "hangs"),
        ];
        let (promoted, rejections) = promote(&assertions);
        assert_eq!(promoted.len(), 2);
        assert_eq!(
            rejections,
            [(1, Rejection::NotFalsifiable), (2, Rejection::NotAClaim)]
        );
        assert_eq!(promoted[0].input, "[2, 1]");
        assert_eq!(promoted[1].input, "[0]");
    }

    #[test]
    fn rejections_carry_input_indices_and_preserve_order() {
        let assertions = [
            judgement("a", "x"),
            claim("b", "i", "e", "e"),
            claim("c", "", "e", "a"),
            claim("d", "i", "e", "a"),
            claim("d", "i", "e", "a"),
        ];
        let (promoted, rejections) = promote(&assertions);
        assert_eq!(promoted.len(), 1);
        assert_eq!(promoted[0].target, "d");
        assert_eq!(
            rejections,
            [
                (0, Rejection::NotAClaim),
                (1, Rejection::NotFalsifiable),
                (
                    2,
                    Rejection::Incomplete {
                        field: String::from("input")
                    }
                ),
                (4, Rejection::Duplicate),
            ]
        );
    }

    #[test]
    fn fingerprint_ignores_case_and_surrounding_whitespace() {
        assert_eq!(
            fingerprint(" Sort ", "[]", "[]"),
            fingerprint("sort", "  []  ", "  [] ")
        );
        assert_ne!(
            fingerprint("sort", "[]", "[]"),
            fingerprint("sort", "[]", "[0]")
        );
    }

    #[test]
    fn fingerprint_ignores_actual() {
        assert_eq!(
            fingerprint("sort", "[]", "[]"),
            fingerprint("sort", "[]", "[]")
        );
        let first = claim("sort", "[]", "[]", "panics");
        let second = claim("sort", "[]", "[]", "returns garbage");
        let (promoted, rejections) = promote(&[first, second]);
        assert_eq!(promoted.len(), 1);
        assert_eq!(rejections, [(1, Rejection::Duplicate)]);
        assert_eq!(promoted[0].fingerprint, fingerprint("sort", "[]", "[]"));
    }

    #[test]
    fn promote_empty_slice_yields_empty_results() {
        let (promoted, rejections) = promote(&[]);
        assert!(promoted.is_empty());
        assert!(rejections.is_empty());
        assert!(objective_leaks(&[]).is_empty());
    }

    #[test]
    fn promote_preserves_original_spelling() {
        let assertions = [claim(" Sort ", "[]", " [] ", "panics")];
        let (promoted, rejections) = promote(&assertions);
        assert!(rejections.is_empty());
        assert_eq!(promoted.len(), 1);
        assert_eq!(promoted[0].target, " Sort ");
        assert_eq!(promoted[0].expect, " [] ");
    }

    #[test]
    fn verify_quotes_accepts_present_quote_and_rejects_missing_quote() {
        let assertion = claim("target", "input", "expected", "actual \"return 1\"");
        assert_eq!(verify_quotes(&assertion, "fn f() { return 1 }", ""), None);
        assert_eq!(
            verify_quotes(&assertion, "fn f() { return 2 }", ""),
            Some(Rejection::Fabricated {
                quote: String::from("return 1")
            })
        );
    }

    #[test]
    fn verify_quotes_projects_quote_from_critic() {
        let assertion = claim("rival", "input", "expected", "actual \"line.replace()\"");
        assert_eq!(
            verify_quotes(
                &assertion,
                "fn rival() {}",
                "fn critic() { line.replace() }"
            ),
            Some(Rejection::Projected {
                quote: String::from("line.replace()")
            })
        );
    }

    #[test]
    fn projected_quote_wins_over_fabricated_quote() {
        let assertion = claim(
            "rival",
            "input",
            "expected",
            "\"missing first\" then \"critic line\"",
        );
        assert_eq!(
            verify_quotes(&assertion, "fn rival() {}", "critic line"),
            Some(Rejection::Projected {
                quote: String::from("critic line")
            })
        );
    }

    #[test]
    fn verify_quotes_handles_boundaries_and_ignored_fragments() {
        let no_quotes = claim("t", "i", "e", "plain prose");
        assert_eq!(verify_quotes(&no_quotes, "source", "critic"), None);

        let ignored = claim("t", "i", "e", "\"\" \"  \" and \"present\"");
        assert_eq!(verify_quotes(&ignored, "present", ""), None);

        let later_missing = claim("t", "i", "e", "\"first\" then \"second\"");
        assert_eq!(
            verify_quotes(&later_missing, "first", ""),
            Some(Rejection::Fabricated {
                quote: String::from("second")
            })
        );

        let unterminated = claim("t", "i", "e", "\"present\" and \"unfinished");
        assert_eq!(verify_quotes(&unterminated, "present", ""), None);
    }

    #[test]
    fn verify_quotes_requires_a_supplied_subject_and_exact_spelling() {
        let assertion = claim("t", "i", "e", "\"a  b\"");
        assert_eq!(verify_quotes(&assertion, "", ""), None);
        assert_eq!(
            verify_quotes(&assertion, "a b", ""),
            Some(Rejection::Fabricated {
                quote: String::from("a  b")
            })
        );
        assert_eq!(verify_quotes(&assertion, "", "a  b"), None);
    }

    #[test]
    fn verify_quotes_ignores_judgements_and_empty_critic_cannot_project() {
        let judgement = judgement("t", "\"missing\"");
        assert_eq!(verify_quotes(&judgement, "source", "missing"), None);

        let assertion = claim("t", "i", "e", "\"missing\"");
        assert_eq!(
            verify_quotes(&assertion, "source", ""),
            Some(Rejection::Fabricated {
                quote: String::from("missing")
            })
        );
    }

    #[test]
    fn verify_all_is_empty_for_empty_input_and_preserves_refusal_order() {
        assert!(verify_all(&[], "source", "critic").is_empty());
        let assertions = [
            claim("a", "i", "e", "\"one\""),
            claim("b", "i", "e", "\"two\""),
            judgement("c", "\"three\""),
        ];
        assert_eq!(
            verify_all(&assertions, "one", "two"),
            vec![(
                1,
                Rejection::Projected {
                    quote: String::from("two")
                }
            ),]
        );
    }
}
