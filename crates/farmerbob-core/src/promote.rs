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
//! [`coverage`] answers a third question, about the critic rather than the
//! claim: whether its citations reach past the head of the subject.

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

/// Backticks in a run of this many or more open or close a fenced block.
///
/// Three is the Markdown fence; longer runs fence a block that itself
/// contains fences, and are given the same treatment.
const FENCE_BACKTICKS: usize = 3;

/// Length of the run of backticks beginning at `start` of `bytes`.
fn backtick_run(bytes: &[u8], start: usize) -> usize {
    let mut run = 0;
    while start + run < bytes.len() && bytes[start + run] == b'`' {
        run += 1;
    }
    run
}

/// Every quoted fragment in `text`, in source order.
///
/// A fragment is a run of text between two `"` characters, or between two
/// single backticks. A run of [`FENCE_BACKTICKS`] or more backticks opens or
/// closes a fenced block instead: fences are not fragment delimiters, and
/// nothing between two fences is extracted, because fenced blocks hold whole
/// functions that legitimately appear in both files. Inside a fragment, the
/// other delimiter kind is ordinary text, so no fragment spans two kinds of
/// delimiter. Empty and whitespace-only fragments are omitted, and an
/// unterminated final fragment is omitted.
///
/// The returned vector is empty exactly when no fragment was extracted, which
/// is a statement about delimiters only — it is not a verdict on the claim;
/// ask [`verify_quotes`] for that. Order is source order, and duplicates are
/// kept.
fn quoted_fragments(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut fragments: Vec<&str> = Vec::new();
    // Delimiter that must close the open fragment, and where its text starts.
    let mut open: Option<(u8, usize)> = None;
    // True between a fence-opening backtick run and its closing run.
    let mut fenced = false;
    let mut i = 0;
    while i < bytes.len() {
        if fenced {
            if bytes[i] == b'`' {
                let run = backtick_run(bytes, i);
                if run >= FENCE_BACKTICKS {
                    fenced = false;
                }
                i += run;
            } else {
                i += 1;
            }
            continue;
        }
        match open {
            Some((delimiter, start)) => {
                if bytes[i] == delimiter {
                    let fragment = &text[start..i];
                    if !fragment.trim().is_empty() {
                        fragments.push(fragment);
                    }
                    open = None;
                }
                i += 1;
            }
            None => match bytes[i] {
                b'"' => {
                    open = Some((b'"', i + 1));
                    i += 1;
                }
                b'`' => {
                    let run = backtick_run(bytes, i);
                    if run >= FENCE_BACKTICKS {
                        fenced = true;
                        i += run;
                    } else {
                        open = Some((b'`', i + 1));
                        i += 1;
                    }
                }
                _ => i += 1,
            },
        }
    }
    fragments
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
/// Fragments are runs between paired `"` characters or between paired single
/// backticks — the two ways this harness's critics have actually quoted code.
/// That pair of delimiters is a known subset of how a critic may quote: text
/// marked some other way is simply not extracted, and the claim passes
/// unrefused. That is the safe direction of failure, because a missed
/// extraction costs one undetected projection, while extracting something that
/// is not evidence would refuse a true claim. Triple-backtick fenced blocks are
/// never evidence: see [`quoted_fragments`]. Empty and whitespace-only
/// fragments are ignored, and an unterminated final fragment is ignored.
/// Matching is exact and byte-for-byte by design; guessing at equivalent
/// whitespace or spelling would silently permit fabrication.
pub fn verify_quotes(a: &Assertion, subject: &str, critic: &str) -> Option<Rejection> {
    let Assertion::Claim { actual, .. } = a else {
        return None;
    };
    if subject.is_empty() {
        return None;
    }

    let mut first_fabricated: Option<String> = None;
    let mut first_projected: Option<String> = None;
    for quote in quoted_fragments(actual) {
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

/// How thoroughly a critic's citations cover the file they are about.
///
/// A closed set of two, like `Verdict` and [`Rejection`]: every judgement of
/// a critic's reach is one of these variants and no others. New shapes of
/// partial reading extend this enum; they do not grow parallel ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Coverage {
    /// Citations reach past the head of the file, or there is not enough evidence to judge.
    Unremarkable,
    /// Every cited line sits in the head of a large file: the critic appears to have read
    /// the opening and inferred the rest. The claims may still be true.
    HeadOnly {
        /// The largest line number cited.
        deepest_cited: u32,
        /// Total lines in the subject.
        subject_lines: u32,
    },
}

/// Fewer than this many distinct citations cannot establish a pattern.
///
/// A reviewer who finds one real defect on line 40 and says so has done
/// nothing wrong; one or two shallow citations are ordinary reviewing, not
/// evidence of a partial read.
const MIN_DISTINCT_CITATIONS: usize = 3;

/// A subject shorter than this has no "rest of the file" to have missed.
///
/// Clustering in the first quarter of a 60-line file is meaningless: there
/// is nothing below the head to have skipped.
const MIN_SUBJECT_LINES: u32 = 400;

/// Every citation must sit at or below this percent of the subject.
///
/// A quarter of the file is a generous definition of "head": the real
/// incident clustered at 17% of an 1892-line file. One citation beyond this
/// line proves the critic read past the opening, so the deepest citation
/// alone decides the boundary, and it is inclusive: exactly 25% is still
/// head-only, one line past 25% is not.
const HEAD_PERCENT: u64 = 25;

/// Turns a line count into a percent for the head test.
///
/// Comparing `line * PERCENT_SCALE` against `subject * HEAD_PERCENT` needs
/// no division, so a zero-length subject cannot divide by zero, and `u64`
/// leaves room for every `u32` pair.
const PERCENT_SCALE: u64 = 100;

/// True when `line` lies strictly beyond `HEAD_PERCENT` percent of `subject`.
fn beyond_head(line: u32, subject: u32) -> bool {
    u64::from(line) * PERCENT_SCALE > u64::from(subject) * HEAD_PERCENT
}

/// Judge how far a critic's citations reach into the file.
///
/// This is the seventh known failure signal, and deliberately not a refusal:
/// it describes the critic's coverage, not the claim's truth. A claim
/// written after reading only the opening of a file may still be true —
/// finding a genuine defect near the top is ordinary reviewing — so a
/// [`Coverage::HeadOnly`] verdict must never remove a claim from the
/// promoted list. It records a suspicion about the reader, not a defect in
/// the code.
///
/// The ways a critique can be shallow are open, and this function recognises
/// exactly one of them: citations clustered in the head of the file. A
/// critic that reads the whole file and then reasons badly about it is not
/// detected here, and is not meant to be.
///
/// The verdict is [`Coverage::HeadOnly`] only when all three thresholds
/// hold: at least 3 distinct citations (`MIN_DISTINCT_CITATIONS` — one or
/// two cannot establish a pattern); a subject of at least 400 lines
/// (`MIN_SUBJECT_LINES` — a short file has no rest to have missed); and
/// every citation at or below 25% of the subject (`HEAD_PERCENT` — the real
/// incident sat at 17%). The deepest citation decides the boundary, because
/// one line read at the bottom proves the critic got there.
///
/// An empty `cited_lines` is [`Coverage::Unremarkable`], NOT
/// [`Coverage::HeadOnly`]: no citations is not evidence of a shallow read;
/// it is no evidence at all. A `subject_lines` of zero is also
/// [`Coverage::Unremarkable`] — a subject whose length is unknown cannot be
/// judged — and so is any citation greater than `subject_lines`: a stale or
/// wrong citation is not evidence about coverage, and no ratio above one is
/// ever computed.
///
/// `cited_lines` is treated as a set: order does not matter, duplicates
/// collapse into one citation, and a cited line of zero is an ordinary
/// member. All arithmetic is integer arithmetic.
pub fn coverage(cited_lines: &[u32], subject_lines: u32) -> Coverage {
    let mut distinct = cited_lines.to_vec();
    distinct.sort_unstable();
    distinct.dedup();

    let Some(&deepest) = distinct.last() else {
        return Coverage::Unremarkable;
    };

    let head_only = distinct.len() >= MIN_DISTINCT_CITATIONS
        && subject_lines >= MIN_SUBJECT_LINES
        && deepest <= subject_lines
        && !beyond_head(deepest, subject_lines);
    if head_only {
        Coverage::HeadOnly {
            deepest_cited: deepest,
            subject_lines,
        }
    } else {
        Coverage::Unremarkable
    }
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
    fn backtick_quoted_projection_from_the_pareto_tokens_incident() {
        // pareto-tokens, 2026-09-17: the critic described its own
        // implementation in inline backticks, and a double-quote-only scan
        // extracted nothing at all.
        let actual = "ACTUAL: `Launcher::Unknown` (the code only examines \
                      `Source::launcher`, with a special-case fallback).";
        let assertion = claim(
            "glm-53-flash",
            "arm=codex-luna",
            "routes via the registry",
            actual,
        );
        let subject = "let chosen = registry.cmd.get(arm).copied();";
        let critic = "fn fallback(arm: &str) -> Launcher {\n    \
                      // the code only examines Source::launcher\n    Launcher::Unknown\n}";
        assert_eq!(
            verify_quotes(&assertion, subject, critic),
            Some(Rejection::Projected {
                quote: String::from("Launcher::Unknown")
            })
        );
    }

    #[test]
    fn backtick_fragments_present_in_subject_are_accepted() {
        let assertion = claim(
            "t",
            "i",
            "e",
            "the subject reads `registry.cmd.get(arm)` and keeps an `Option<Arm>`",
        );
        let subject = "let held: Option<Arm> = registry.cmd.get(arm).copied();";
        assert_eq!(verify_quotes(&assertion, subject, "unrelated critic"), None);
    }

    #[test]
    fn backtick_fragment_absent_from_both_sources_is_fabricated() {
        let assertion = claim("t", "i", "e", "it builds `Launcher::Never` instead");
        assert_eq!(
            verify_quotes(&assertion, "fn f() {}", "fn g() {}"),
            Some(Rejection::Fabricated {
                quote: String::from("Launcher::Never")
            })
        );
    }

    #[test]
    fn triple_backtick_fence_is_not_evidence() {
        // A fenced block holds a whole function that legitimately appears in
        // both files; its text must not refuse a claim even when absent from
        // the subject and present in the critic.
        let actual = "```rust\nfn absent_from_subject() -> u8 { 9 }\n```";
        let assertion = claim("t", "i", "e", actual);
        let critic = "fn absent_from_subject() -> u8 { 9 }";
        assert_eq!(verify_quotes(&assertion, "fn f() {}", critic), None);
    }

    #[test]
    fn mixed_fragments_refuse_on_the_absent_one_verbatim() {
        let double_present = claim("t", "i", "e", "\"present\" then `absent`");
        assert_eq!(
            verify_quotes(&double_present, "present", "absent"),
            Some(Rejection::Projected {
                quote: String::from("absent")
            })
        );

        let backtick_present = claim("t", "i", "e", "`present` then \"absent\"");
        assert_eq!(
            verify_quotes(&backtick_present, "present", "unrelated"),
            Some(Rejection::Fabricated {
                quote: String::from("absent")
            })
        );
    }

    #[test]
    fn one_delimiter_inside_the_other_does_not_span_a_fragment() {
        // A double quote inside a backtick fragment stays inside it...
        let quoted = claim("t", "i", "e", "he wrote `say \"hi\" now` loudly");
        assert_eq!(
            verify_quotes(&quoted, "nothing here", "say \"hi\" now"),
            Some(Rejection::Projected {
                quote: String::from("say \"hi\" now")
            })
        );
        // ...and a backtick inside a double-quoted fragment stays inside that.
        let backticked = claim("t", "i", "e", "wrote \"use `fmt` here\" ok");
        assert_eq!(
            verify_quotes(&backticked, "nothing here", "use `fmt` here"),
            Some(Rejection::Projected {
                quote: String::from("use `fmt` here")
            })
        );
    }

    #[test]
    fn unterminated_backtick_yields_no_fragment_and_does_not_panic() {
        // Odd count: the trailing backtick opens a fragment that never
        // closes, so `dangling` is never checked even though the critic has it.
        let mixed = claim("t", "i", "e", "`present` and `dangling");
        assert_eq!(verify_quotes(&mixed, "present", "dangling"), None);

        let lone = claim("t", "i", "e", "ends with a lone ` tick");
        assert_eq!(verify_quotes(&lone, "tick", "tick"), None);
    }

    #[test]
    fn empty_and_whitespace_backtick_fragments_are_ignored() {
        let assertion = claim("t", "i", "e", "`` plus `present` and `  ` end");
        assert_eq!(verify_quotes(&assertion, "present", ""), None);
    }

    #[test]
    fn adjacent_fragments_with_nothing_between_are_all_extracted() {
        let backticks = claim("t", "i", "e", "`a``b`");
        assert_eq!(
            verify_quotes(&backticks, "a", "b"),
            Some(Rejection::Projected {
                quote: String::from("b")
            })
        );

        let doubles = claim("t", "i", "e", "\"a\"\"b\"");
        assert_eq!(
            verify_quotes(&doubles, "a", "b"),
            Some(Rejection::Projected {
                quote: String::from("b")
            })
        );
    }

    #[test]
    fn backtick_matching_is_exact_interior_whitespace_is_never_trimmed() {
        let assertion = claim("t", "i", "e", "returns `a  b` on collapse");
        // One space in the subject: not a match, quote carried verbatim.
        assert_eq!(
            verify_quotes(&assertion, "x a b y", ""),
            Some(Rejection::Fabricated {
                quote: String::from("a  b")
            })
        );
        // Two spaces: exact match.
        assert_eq!(verify_quotes(&assertion, "x a  b y", ""), None);
    }

    #[test]
    fn the_first_fragment_in_source_order_is_the_one_reported() {
        // Both fragments are projections; the earlier one is carried.
        let assertion = claim("t", "i", "e", "`zebra` then `yak`");
        assert_eq!(
            verify_quotes(&assertion, "neither here", "zebra yak"),
            Some(Rejection::Projected {
                quote: String::from("zebra")
            })
        );
    }

    #[test]
    fn quoted_fragments_order_and_empty_result_are_pinned() {
        // No fragments in, empty vector out: a statement about delimiters,
        // not a verdict on the claim.
        assert!(quoted_fragments("").is_empty());
        assert!(quoted_fragments("plain prose, no delimiters").is_empty());
        assert!(quoted_fragments("```fenced\nblock\n```").is_empty());
        // Source order, mixed delimiters, duplicates kept.
        assert_eq!(
            quoted_fragments("\"b\" then `a` then `a`"),
            vec!["b", "a", "a"]
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

    #[test]
    fn the_real_incident_is_head_only() {
        assert_eq!(
            coverage(&[67, 74, 152, 310], 1892),
            Coverage::HeadOnly {
                deepest_cited: 310,
                subject_lines: 1892
            }
        );
    }

    #[test]
    fn citations_spread_through_the_file_are_unremarkable() {
        assert_eq!(coverage(&[67, 900, 1500], 1892), Coverage::Unremarkable);
    }

    #[test]
    fn one_deep_citation_clears_the_verdict() {
        assert_eq!(coverage(&[67, 74, 1800], 1892), Coverage::Unremarkable);
    }

    #[test]
    fn two_citations_are_never_head_only() {
        // Even two shallow citations of a large file: a pattern needs three.
        assert_eq!(coverage(&[5, 10], 5000), Coverage::Unremarkable);
        assert_eq!(coverage(&[1, 2], 1892), Coverage::Unremarkable);
    }

    #[test]
    fn three_citations_exactly_are_eligible() {
        assert_eq!(
            coverage(&[67, 74, 152], 1892),
            Coverage::HeadOnly {
                deepest_cited: 152,
                subject_lines: 1892
            }
        );
    }

    #[test]
    fn short_subjects_are_unremarkable_whatever_the_citations() {
        assert_eq!(coverage(&[67, 74, 152], 300), Coverage::Unremarkable);
        // Boundary: 399 lines is short, 400 is not, for the same citations.
        assert_eq!(coverage(&[1, 50, 99], 399), Coverage::Unremarkable);
        assert_eq!(
            coverage(&[1, 50, 99], 400),
            Coverage::HeadOnly {
                deepest_cited: 99,
                subject_lines: 400
            }
        );
    }

    #[test]
    fn empty_citations_are_no_evidence_at_all() {
        assert_eq!(coverage(&[], 1892), Coverage::Unremarkable);
        assert_eq!(coverage(&[], 0), Coverage::Unremarkable);
    }

    #[test]
    fn unknown_subject_length_cannot_be_judged() {
        assert_eq!(coverage(&[1, 2, 3], 0), Coverage::Unremarkable);
    }

    #[test]
    fn stale_citation_past_the_end_is_unremarkable() {
        assert_eq!(coverage(&[1, 2, 2000], 1892), Coverage::Unremarkable);
    }

    #[test]
    fn head_boundary_pins_both_sides_of_25_percent() {
        // 100 of 400 is exactly 25%: still head-only.
        assert_eq!(
            coverage(&[1, 50, 100], 400),
            Coverage::HeadOnly {
                deepest_cited: 100,
                subject_lines: 400
            }
        );
        // One line past 25%: not.
        assert_eq!(coverage(&[1, 50, 101], 400), Coverage::Unremarkable);
    }

    #[test]
    fn duplicates_and_zero_behave_as_the_same_set() {
        // Duplicates collapse: the same evidence as [67, 74, 152], said twice.
        assert_eq!(
            coverage(&[67, 67, 74, 152], 1892),
            coverage(&[67, 74, 152], 1892)
        );
        // After collapsing, the set has two members, so no pattern.
        assert_eq!(coverage(&[67, 67, 74], 1892), Coverage::Unremarkable);
        // Zero is an ordinary member: harmless, and it counts once.
        assert_eq!(coverage(&[0, 0, 67], 1892), Coverage::Unremarkable);
        assert_eq!(
            coverage(&[0, 67, 74], 1892),
            Coverage::HeadOnly {
                deepest_cited: 74,
                subject_lines: 1892
            }
        );
    }

    #[test]
    fn citation_order_does_not_matter() {
        assert_eq!(
            coverage(&[310, 152, 74, 67], 1892),
            coverage(&[67, 74, 152, 310], 1892)
        );
    }

    #[test]
    fn u32_max_subject_does_not_overflow() {
        assert_eq!(
            coverage(&[1, 2, 3], u32::MAX),
            Coverage::HeadOnly {
                deepest_cited: 3,
                subject_lines: u32::MAX
            }
        );
        // A citation at the very end of a maximal file: 100%, beyond the
        // head, and the percent arithmetic must not overflow.
        assert_eq!(
            coverage(&[1, 2, u32::MAX], u32::MAX),
            Coverage::Unremarkable
        );
    }

    #[test]
    fn head_only_is_a_suspicion_and_never_a_rejection() {
        // A head-only reading does not stop the claims underneath it from
        // being promoted: coverage judges the critic, not the claim.
        let verdict = coverage(&[67, 74, 152], 1892);
        assert_eq!(
            verdict,
            Coverage::HeadOnly {
                deepest_cited: 152,
                subject_lines: 1892
            }
        );
        let assertions = [
            claim("sort", "[]", "[]", "panics"),
            claim("sort", "[1]", "[1]", "hangs"),
            claim("sort", "[2]", "[2]", "corrupts"),
        ];
        let (promoted, rejections) = promote(&assertions);
        assert_eq!(promoted.len(), 3);
        assert!(rejections.is_empty());
    }
}

// ESCALATED: 2 confirmed finding(s) by critic glm-53-flash, found on ifm-k2-horizon.
// Promoted from an executed proof that passed the reference veto. Provenance is
// recorded so a bad test can be traced and retired.  (bead farmerbob-mqr)
#[cfg(test)]
mod escalated_backtick_quotes_ifm_k2_horizon {
    use super::*;

    #[test]
    fn claim_1() {
        // "The mixed boundary is unenforced: an absent backtick fragment beside a present double-quoted one is not refused."
        let assertion = Assertion::Claim {
            target: String::from("target"),
            input: String::from("input"),
            expect: String::from("refused"),
            actual: String::from("\"registry.cmd.get(arm)\" then `Source::launcher`"),
        };
        assert_eq!(
            verify_quotes(
                &assertion,
                "registry.cmd.get(arm)",
                "critic mentions Source::launcher"
            ),
            Some(Rejection::Projected {
                quote: String::from("Source::launcher")
            })
        );
    }

    #[test]
    fn claim_2() {
        // "`verify_all` likewise returns no refusal for a backtick-only projection."
        let assertion = Assertion::Claim {
            target: String::from("target"),
            input: String::from("input"),
            expect: String::from("refused"),
            actual: String::from("\"registry.cmd.get(arm)\" then `Source::launcher`"),
        };
        assert_eq!(
            verify_all(
                &[assertion],
                "registry.cmd.get(arm)",
                "critic mentions Source::launcher"
            ),
            vec![(
                0,
                Rejection::Projected {
                    quote: String::from("Source::launcher")
                }
            )]
        );
    }
}
