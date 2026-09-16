//! Per-adapter usage-limit signal recognition.
//!
//! [`quota`](crate::quota) models what to *do* about a limit (parking buckets,
//! resumption handles). This module models how to *recognise* one: each provider
//! CLI or API reports "out of quota" differently (an exit code, a stderr line, a
//! JSON error field, an HTTP 429 surfaced mid-stream), so recognition rules are
//! data ([`SignalRules`]) rather than code. A new adapter is a new value, never
//! a new match arm.
//!
//! The distinction that matters: a run killed by a quota cap is not a run the
//! agent failed at. Conflating them charges a model for its provider's billing
//! policy, in either direction.
//!
//! Everything here is pure logic: no I/O, no network, no clock reads. The
//! current time enters only as the `now` parameter, used to resolve *relative*
//! reset statements ("try again in 3600 seconds") into absolute instants.

/// What the harness concluded about one finished run, from its exit status and output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classification {
    /// Ran to completion. Not a limit.
    Normal,
    /// Provider refused on quota grounds. Carries the reset instant when one was stated,
    /// as a unix timestamp in seconds.
    Limited {
        /// Absolute reset instant in unix seconds, when the output stated one.
        ///
        /// [`None`] means no reset time was stated. The caller applies its own
        /// fallback window then; a guessed instant must never masquerade as a
        /// measured one.
        reset_at: Option<u64>,
        /// The output text that matched, for a human deciding whether the
        /// classifier is right.
        evidence: String,
    },
    /// Something matched a limit-shaped pattern but the rule set is not confident.
    /// The caller must not park a bucket on this alone.
    Ambiguous {
        /// The output text that matched, for a human deciding whether the
        /// classifier is right.
        evidence: String,
    },
}

/// One provider's recognition rules. Data, not code: a new adapter is a new value,
/// never a new match arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalRules {
    /// Exit codes that mean quota, e.g. `vec![429]`.
    pub limit_exit_codes: Vec<i32>,
    /// Case-insensitive substrings of stderr/stdout that mean quota.
    pub limit_patterns: Vec<String>,
    /// Substrings that look like a limit but are NOT one, checked first.
    /// e.g. "rate limit" inside a model's own explanatory prose.
    pub exclusions: Vec<String>,
    /// Fallback window when a limit is detected but no reset time is stated.
    ///
    /// Stored for the caller to apply; classification itself never substitutes
    /// this for a missing reset instant.
    pub default_window_secs: u64,
}

impl SignalRules {
    /// Creates an empty rule set with the given fallback window in seconds.
    pub fn new(default_window_secs: u64) -> Self {
        Self {
            limit_exit_codes: Vec::new(),
            limit_patterns: Vec::new(),
            exclusions: Vec::new(),
            default_window_secs,
        }
    }

    /// Adds an exit code that means quota.
    pub fn with_exit_code(mut self, code: i32) -> Self {
        self.limit_exit_codes.push(code);
        self
    }

    /// Adds a case-insensitive output substring that means quota.
    pub fn with_pattern(mut self, pat: &str) -> Self {
        self.limit_patterns.push(pat.to_string());
        self
    }

    /// Adds a case-insensitive output substring that cancels a limit reading.
    pub fn with_exclusion(mut self, pat: &str) -> Self {
        self.exclusions.push(pat.to_string());
        self
    }
}

/// Classify one finished run.
///
/// `now` is the current unix time in seconds, used only to resolve *relative* reset
/// statements ("try again in 3600 seconds") into absolute instants.
///
/// The decision order is: exclusions first (any hit means [`Classification::Normal`],
/// even when a limit pattern also matched), then limit patterns (a hit with exit
/// code `0` is [`Classification::Ambiguous`], otherwise [`Classification::Limited`]),
/// then limit exit codes (a hit alone suffices for [`Classification::Limited`]).
/// Anything else is [`Classification::Normal`]: ordinary failure is the common
/// case and must not pollute the quota path.
pub fn classify(rules: &SignalRules, exit_code: i32, output: &str, now: u64) -> Classification {
    if rules
        .exclusions
        .iter()
        .any(|exclusion| find_insensitive(output, exclusion).is_some())
    {
        return Classification::Normal;
    }
    if let Some(evidence) = first_match_text(output, &rules.limit_patterns) {
        if exit_code == 0 {
            return Classification::Ambiguous { evidence };
        }
        return Classification::Limited {
            reset_at: parse_reset(output, now),
            evidence,
        };
    }
    if rules.limit_exit_codes.contains(&exit_code) {
        let evidence = if output.is_empty() {
            format!("exit code {exit_code}")
        } else {
            output.to_string()
        };
        return Classification::Limited {
            reset_at: parse_reset(output, now),
            evidence,
        };
    }
    Classification::Normal
}

/// Extract an absolute reset instant from free text, if one is stated.
///
/// Recognises, at minimum:
///   - `retry-after: 3600`              (seconds from `now`)
///   - `"reset_at": 1789000000`         (absolute unix seconds)
///   - `resets at 2026-09-17T00:00:00Z` (RFC 3339)
///
/// Returns [`None`] rather than guessing on unparseable input. A *relative*
/// statement never resolves to an instant in the past relative to `now`.
pub fn parse_reset(output: &str, now: u64) -> Option<u64> {
    parse_reset_at_field(output)
        .or_else(|| parse_resets_at_timestamp(output))
        .or_else(|| parse_relative_seconds(output, now))
}

// Returns the exact output slice matched by the first matching pattern,
// preserving the output's original casing for human review.
fn first_match_text(output: &str, patterns: &[String]) -> Option<String> {
    patterns.iter().find_map(|pattern| {
        let (start, end) = find_insensitive(output, pattern)?;
        output.get(start..end).map(str::to_string)
    })
}

// Finds `needle` in `haystack` case-insensitively, returning the byte range of
// the match within `haystack`. Empty needles never match, so empty rules are
// inert. Byte indices come from `char_indices`, so slicing at them is safe.
fn find_insensitive(haystack: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    let needle_chars: Vec<char> = needle.chars().collect();
    let hay_chars: Vec<(usize, char)> = haystack.char_indices().collect();
    if hay_chars.len() < needle_chars.len() {
        return None;
    }
    let mut start_index = 0;
    while start_index + needle_chars.len() <= hay_chars.len() {
        let mut matches = true;
        let mut offset = 0;
        while offset < needle_chars.len() {
            match (hay_chars.get(start_index + offset), needle_chars.get(offset)) {
                (Some((_, hay)), Some(need)) => {
                    if *hay != *need && !chars_equal_insensitive(*hay, *need) {
                        matches = false;
                        break;
                    }
                }
                _ => {
                    matches = false;
                    break;
                }
            }
            offset += 1;
        }
        if matches {
            match hay_chars.get(start_index) {
                Some((start_byte, _)) => {
                    let end_byte = match hay_chars.get(start_index + needle_chars.len()) {
                        Some((byte, _)) => *byte,
                        None => haystack.len(),
                    };
                    return Some((*start_byte, end_byte));
                }
                None => break,
            }
        }
        start_index += 1;
    }
    None
}

// Case-insensitive character equality with an ASCII fast path.
fn chars_equal_insensitive(left: char, right: char) -> bool {
    if left == right {
        return true;
    }
    if left.is_ascii() && right.is_ascii() {
        return left.eq_ignore_ascii_case(&right);
    }
    left.to_lowercase().collect::<String>() == right.to_lowercase().collect::<String>()
}

// Byte end-offsets (within `text`) just past every case-insensitive occurrence
// of `keyword`. Used to scan all candidate values, not just the first.
fn match_ends(text: &str, keyword: &str) -> Vec<usize> {
    let mut ends = Vec::new();
    let mut rest = text;
    let mut base = 0usize;
    while let Some((_, end)) = find_insensitive(rest, keyword) {
        base = base.saturating_add(end);
        ends.push(base);
        match rest.get(end..) {
            Some(next) if !next.is_empty() => rest = next,
            _ => break,
        }
    }
    ends
}

// Drops the leading characters satisfying `pred`, returning the remainder.
fn skip_while(text: &str, pred: impl Fn(char) -> bool) -> &str {
    match text.char_indices().find(|(_, c)| !pred(*c)) {
        Some((index, _)) => text.get(index..).unwrap_or_default(),
        None => "",
    }
}

// The leading run of ASCII digits in `text` (possibly empty).
fn leading_digits(text: &str) -> &str {
    let mut end = 0usize;
    for (index, c) in text.char_indices() {
        if c.is_ascii_digit() {
            end = index.saturating_add(c.len_utf8());
        } else {
            break;
        }
    }
    text.get(..end).unwrap_or_default()
}

// Relative statements ("retry-after: 3600", "try again in 3600 seconds",
// "reset in 30") resolved against `now`. Saturating addition keeps the result
// out of the past even for `0` or saturating inputs.
fn parse_relative_seconds(output: &str, now: u64) -> Option<u64> {
    const KEYWORDS: &[&str] = &[
        "retry-after",
        "retry_after",
        "retryafter",
        "retry after",
        "try again in",
        "retry in",
        "resets in",
        "reset in",
    ];
    for keyword in KEYWORDS {
        for end in match_ends(output, keyword) {
            let mut tail = match output.get(end..) {
                Some(tail) => tail,
                None => continue,
            };
            tail = skip_while(tail, |c| {
                c == ' ' || c == '\t' || c == '\r' || c == '\n' || c == ':' || c == '='
            });
            if tail.len() > 3
                && tail
                    .get(..3)
                    .is_some_and(|head| head.eq_ignore_ascii_case("in "))
            {
                tail = tail.get(3..).unwrap_or("");
            }
            let digits = leading_digits(tail);
            if digits.is_empty() {
                continue;
            }
            if let Ok(seconds) = digits.parse::<u64>() {
                return Some(now.saturating_add(seconds));
            }
        }
    }
    None
}

// Absolute unix seconds ("reset_at": 1789000000). Returned as stated, even when
// in the past: only relative statements are clamped to the present.
fn parse_reset_at_field(output: &str) -> Option<u64> {
    const KEYWORDS: &[&str] = &["reset_at", "reset-at", "resets_at", "resetat"];
    for keyword in KEYWORDS {
        for end in match_ends(output, keyword) {
            let mut tail = match output.get(end..) {
                Some(tail) => tail,
                None => continue,
            };
            tail = skip_while(tail, |c| c == '"' || c == '\'' || c == ' ' || c == '\t');
            if let Some(rest) = tail.strip_prefix(':').or_else(|| tail.strip_prefix('=')) {
                tail = rest;
            }
            tail = skip_while(tail, |c| c == '"' || c == '\'' || c == ' ' || c == '\t');
            let digits = leading_digits(tail);
            if digits.is_empty() {
                continue;
            }
            if let Ok(instant) = digits.parse::<u64>() {
                return Some(instant);
            }
        }
    }
    None
}

// Characters trimmed from around a timestamp candidate token.
const TIMESTAMP_TRIM: &[char] = &[
    '"', '\'', '(', ')', '[', ']', '{', '}', '<', '>', ',', '.', ';', '!', '?',
];

// Parses an RFC 3339 timestamp into unix seconds.
fn datetime_to_unix(text: &str) -> Option<u64> {
    let parsed = chrono::DateTime::parse_from_rfc3339(text).ok()?;
    let seconds = parsed.timestamp();
    u64::try_from(seconds).ok()
}

// Prefixed RFC 3339 statements ("resets at 2026-09-17T00:00:00Z").
fn parse_resets_at_timestamp(output: &str) -> Option<u64> {
    const KEYWORDS: &[&str] = &["resets at", "reset at"];
    for keyword in KEYWORDS {
        for end in match_ends(output, keyword) {
            let tail = match output.get(end..) {
                Some(tail) => tail,
                None => continue,
            };
            for token in tail.split_whitespace().take(6) {
                let candidate = token.trim_matches(TIMESTAMP_TRIM);
                if candidate.is_empty() {
                    continue;
                }
                if let Some(instant) = datetime_to_unix(candidate) {
                    return Some(instant);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quota_rules() -> SignalRules {
        SignalRules::new(60)
            .with_pattern("rate limit")
            .with_pattern("quota exceeded")
    }

    #[test]
    fn exclusion_beats_pattern() {
        let rules = quota_rules().with_exclusion("prose explanation");
        let output = "Rate Limit hit, but this is only prose explanation of billing";
        assert_eq!(classify(&rules, 1, output, 1000), Classification::Normal);
    }

    #[test]
    fn exclusion_match_is_case_insensitive() {
        let rules = quota_rules().with_exclusion("EXPLANATORY");
        let output = "rate limit reached in this explanatory paragraph";
        assert_eq!(classify(&rules, 1, output, 1000), Classification::Normal);
    }

    #[test]
    fn pattern_match_is_case_insensitive() {
        let rules = quota_rules();
        match classify(&rules, 1, "RATE LIMIT reached", 1000) {
            Classification::Limited { .. } => {}
            other => panic!("expected Limited, got {other:?}"),
        }
    }

    #[test]
    fn exit_code_alone_suffices() {
        let rules = SignalRules::new(60).with_exit_code(429);
        match classify(&rules, 429, "something entirely unrelated broke", 1000) {
            Classification::Limited { .. } => {}
            other => panic!("expected Limited, got {other:?}"),
        }
    }

    #[test]
    fn limit_with_no_reset_yields_none() {
        let rules = quota_rules();
        match classify(&rules, 1, "quota exceeded for this project", 1000) {
            Classification::Limited { reset_at, .. } => assert_eq!(reset_at, None),
            other => panic!("expected Limited, got {other:?}"),
        }
    }

    #[test]
    fn default_window_is_not_substituted() {
        let rules = SignalRules::new(3600).with_pattern("quota");
        match classify(&rules, 2, "quota hit, no time stated", 100) {
            Classification::Limited { reset_at, .. } => assert_eq!(reset_at, None),
            other => panic!("expected Limited, got {other:?}"),
        }
    }

    #[test]
    fn nonzero_exit_with_no_signal_is_normal() {
        let rules = quota_rules();
        assert_eq!(
            classify(&rules, 1, "Traceback: null pointer dereference", 1000),
            Classification::Normal
        );
    }

    #[test]
    fn zero_exit_with_no_signal_is_normal() {
        let rules = quota_rules();
        assert_eq!(classify(&rules, 0, "all work finished", 1000), Classification::Normal);
    }

    #[test]
    fn pattern_match_with_exit_zero_is_ambiguous() {
        let rules = quota_rules();
        match classify(&rules, 0, "rate limit noted but cached result served", 1000) {
            Classification::Ambiguous { evidence } => {
                assert!(evidence.to_lowercase().contains("rate limit"));
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn evidence_contains_matched_text() {
        let rules = quota_rules();
        let output = "FAILED: QUOTA EXCEEDED for project";
        match classify(&rules, 1, output, 1000) {
            Classification::Limited { evidence, .. } => {
                assert!(!evidence.is_empty());
                assert!(output.contains(&evidence));
                assert!(evidence.to_lowercase().contains("quota exceeded"));
            }
            other => panic!("expected Limited, got {other:?}"),
        }
    }

    #[test]
    fn exit_only_evidence_falls_back_to_output() {
        let rules = SignalRules::new(60).with_exit_code(429);
        match classify(&rules, 429, "boom", 7) {
            Classification::Limited { evidence, reset_at } => {
                assert!(!evidence.is_empty());
                assert!(evidence.contains("boom"));
                assert_eq!(reset_at, None);
            }
            other => panic!("expected Limited, got {other:?}"),
        }
    }

    #[test]
    fn parse_reset_retry_after_seconds() {
        assert_eq!(parse_reset("Error 429: retry-after: 3600, slow down", 1000), Some(4600));
    }

    #[test]
    fn parse_reset_retry_after_case_insensitive() {
        assert_eq!(parse_reset("Retry-After: 120", 1000), Some(1120));
    }

    #[test]
    fn parse_reset_try_again_in_seconds() {
        assert_eq!(
            parse_reset("quota hit, please try again in 3600 seconds", 500),
            Some(4100)
        );
    }

    #[test]
    fn parse_reset_reset_at_absolute() {
        assert_eq!(
            parse_reset(r#"{"error": "quota", "reset_at": 1789000000}"#, 1000),
            Some(1789000000)
        );
    }

    #[test]
    fn parse_reset_resets_at_rfc3339() {
        assert_eq!(
            parse_reset("usage limit hit, resets at 2026-09-17T00:00:00Z please wait", 1000),
            Some(1789603200)
        );
    }

    #[test]
    fn parse_reset_garbage_is_none() {
        assert_eq!(parse_reset("all systems nominal", 1000), None);
        assert_eq!(parse_reset("", 1000), None);
        assert_eq!(parse_reset("retry-after: later, maybe", 1000), None);
        assert_eq!(parse_reset("reset_at: unknown", 1000), None);
    }

    #[test]
    fn parse_reset_relative_never_in_past() {
        assert_eq!(parse_reset("retry-after: 0", 1000), Some(1000));
        let huge = parse_reset("retry-after: 5", u64::MAX);
        match huge {
            Some(instant) => assert!(instant >= u64::MAX.saturating_sub(5)),
            None => panic!("expected a saturating instant"),
        }
        assert_eq!(parse_reset("retry-after: 99999999999999999999999999", 1000), None);
    }

    #[test]
    fn empty_rules_never_signal() {
        let rules = SignalRules::new(60);
        assert_eq!(
            classify(&rules, 1, "rate limit quota exceeded 429", 1000),
            Classification::Normal
        );
        assert_eq!(classify(&rules, 429, "rate limit", 1000), Classification::Normal);
    }

    #[test]
    fn empty_pattern_and_exclusion_are_inert() {
        let rules = SignalRules::new(60).with_pattern("").with_exclusion("");
        assert_eq!(classify(&rules, 1, "anything at all", 1000), Classification::Normal);
    }

    #[test]
    fn pattern_matches_substring_not_whole_word() {
        let rules = SignalRules::new(60).with_pattern("quota");
        match classify(&rules, 2, "over-quota usage detected", 1000) {
            Classification::Limited { .. } => {}
            other => panic!("expected Limited, got {other:?}"),
        }
    }

    #[test]
    fn limited_carries_parsed_reset() {
        let rules = quota_rules();
        match classify(&rules, 1, "rate limit hit; retry-after: 60", 1000) {
            Classification::Limited { reset_at, evidence } => {
                assert_eq!(reset_at, Some(1060));
                assert!(!evidence.is_empty());
            }
            other => panic!("expected Limited, got {other:?}"),
        }
    }

    #[test]
    fn exclusion_beats_pattern_even_with_reset_stated() {
        let rules = quota_rules().with_exclusion("discussing");
        let output = "discussing RATE LIMIT policy; retry-after: 60";
        assert_eq!(classify(&rules, 1, output, 1000), Classification::Normal);
    }

    #[test]
    fn parse_reset_absolute_in_past_is_returned_as_stated() {
        assert_eq!(parse_reset(r#""reset_at": 500"#, 1000), Some(500));
    }

    #[test]
    fn parse_reset_rfc3339_with_offset() {
        // 2026-09-17T02:00:00+02:00 is the same instant as 2026-09-17T00:00:00Z.
        assert_eq!(
            parse_reset("resets at 2026-09-17T02:00:00+02:00", 1000),
            Some(1789603200)
        );
    }

    #[test]
    fn parse_reset_reset_at_keyword_case_insensitive() {
        assert_eq!(parse_reset(r#""RESET_AT": 1789000000"#, 1000), Some(1789000000));
    }

    #[test]
    fn ambiguous_carries_matched_text() {
        let rules = quota_rules();
        let output = "QUOTA EXCEEDED in log, but exit status ok";
        match classify(&rules, 0, output, 1000) {
            Classification::Ambiguous { evidence } => {
                assert!(output.contains(&evidence));
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }
}
// Conformance suite for limit-detect, derived from the task specification ALONE.
//
// THREE clauses the spec leaves underdetermined are deliberately NOT tested, because both
// candidates read them differently and both flagged the ambiguity in their handoff:
//   1. whether an exclusion vetoes a configured EXIT CODE (spec scopes exclusions to
//      "a pattern match", and separately calls an exit code "sufficient on its own");
//   2. what `evidence` contains for an exit-code-only detection, where no text matched;
//   3. whether accepting reset formats BEYOND the three required is a defect.
// Scoring an arm on a clause the spec never fixed measures the author's guess at intent,
// not their capability. These belong back in the spec, not in the suite.
#[cfg(test)]
#[allow(unused_imports)]
mod conformance_limit_detect {
    use super::*;

    fn rules() -> SignalRules {
        SignalRules::new(3600)
            .with_exit_code(429)
            .with_pattern("quota exceeded")
            .with_exclusion("as an example of a quota exceeded message")
    }

    // "An exclusion match beats a pattern match ... the run is Normal even when a limit
    //  pattern also matched."
    #[test]
    fn exclusion_beats_pattern() {
        let out = "Here is as an example of a quota exceeded message for the docs";
        assert_eq!(classify(&rules(), 1, out, 1_000), Classification::Normal);
    }

    // "Pattern and exclusion matching is case-insensitive."
    #[test]
    fn matching_is_case_insensitive() {
        match classify(&rules(), 1, "FATAL: QUOTA EXCEEDED on this account", 1_000) {
            Classification::Limited { .. } => {}
            other => panic!("uppercase pattern must match, got {other:?}"),
        }
    }

    // "An exit code in limit_exit_codes is sufficient on its own, with no pattern present."
    #[test]
    fn exit_code_alone_suffices() {
        match classify(&rules(), 429, "no useful output at all", 1_000) {
            Classification::Limited { .. } => {}
            other => panic!("a configured exit code alone must be Limited, got {other:?}"),
        }
    }

    // "A limit detected with no parseable reset time yields reset_at: None -- do NOT
    //  silently substitute default_window_secs."
    #[test]
    fn a_limit_without_a_stated_reset_has_no_reset() {
        match classify(&rules(), 1, "quota exceeded, sorry", 1_000) {
            Classification::Limited { reset_at, .. } => assert_eq!(
                reset_at, None,
                "default_window_secs must not be substituted for a measurement"
            ),
            other => panic!("expected Limited, got {other:?}"),
        }
    }

    // "A nonzero exit code with no limit signal at all is Normal, not Ambiguous."
    #[test]
    fn ordinary_failure_is_normal() {
        assert_eq!(
            classify(&rules(), 1, "thread 'main' panicked at src/main.rs:4", 1_000),
            Classification::Normal
        );
    }

    // "Ambiguous is returned when a limit pattern matches but the exit code is 0."
    #[test]
    fn pattern_with_success_exit_is_ambiguous() {
        match classify(&rules(), 0, "warning: quota exceeded soon", 1_000) {
            Classification::Ambiguous { .. } => {}
            other => panic!("a pattern hit on a successful run is Ambiguous, got {other:?}"),
        }
    }

    // "evidence contains the matched text, not a generic message." Asserted only for the
    // PATTERN case, which the spec fixes; the exit-code-only shape is ambiguous (see above).
    #[test]
    fn evidence_carries_the_matched_text() {
        match classify(&rules(), 1, "ERROR: Quota Exceeded for org 42", 1_000) {
            Classification::Limited { evidence, .. } => {
                let e = evidence.to_lowercase();
                assert!(
                    e.contains("quota exceeded"),
                    "evidence must quote what matched, got {evidence:?}"
                );
            }
            other => panic!("expected Limited, got {other:?}"),
        }
    }

    // "retry-after: 3600  (seconds from now)"
    #[test]
    fn parse_reset_reads_retry_after_seconds() {
        assert_eq!(parse_reset("retry-after: 3600", 1_000), Some(4_600));
    }

    // "\"reset_at\": 1789000000  (absolute unix seconds)"
    #[test]
    fn parse_reset_reads_an_absolute_stamp() {
        assert_eq!(
            parse_reset(r#"{"reset_at": 1789000000}"#, 1_000),
            Some(1_789_000_000)
        );
    }

    // "resets at 2026-09-17T00:00:00Z  (RFC 3339)"
    #[test]
    fn parse_reset_reads_rfc3339() {
        assert_eq!(
            parse_reset("resets at 2026-09-17T00:00:00Z", 1_000),
            Some(1_789_603_200)
        );
    }

    // "parse_reset returns None rather than guessing on unparseable input."
    #[test]
    fn parse_reset_refuses_to_guess() {
        for junk in ["", "no reset here", "later", "soon-ish", "retry-after: eventually"] {
            assert_eq!(parse_reset(junk, 1_000), None, "must not guess from {junk:?}");
        }
    }

    // "... and never returns an instant in the past relative to now for a relative statement."
    #[test]
    fn a_relative_reset_is_never_in_the_past() {
        if let Some(t) = parse_reset("retry-after: 60", 5_000) {
            assert!(t >= 5_000, "a relative reset must not resolve into the past");
        }
    }

    // "A new adapter is a new value, never a new match arm."
    #[test]
    fn rules_are_data_and_compose() {
        let r = SignalRules::new(60)
            .with_exit_code(7)
            .with_pattern("slow down")
            .with_exclusion("do not slow down");
        match classify(&r, 7, "", 0) {
            Classification::Limited { .. } => {}
            other => panic!("a freshly composed rule set must work, got {other:?}"),
        }
        assert_eq!(classify(&r, 1, "please do not slow down", 0), Classification::Normal);
    }
}



// ESCALATED: 2 confirmed finding(s) by critic or-muse-spark, found on glm-53-flash.
// Promoted from an executed proof that passed the reference veto. Provenance is
// recorded so a bad test can be traced and retired.  (bead farmerbob-mqr)
#[cfg(test)]
    mod escalated_limit_detect_glm_53_flash {
        use super::*;
        // claim_1: Trailing sentence period breaks RFC3339 reset recognition.
        #[test]
        fn claim_1() {
            let now = 1_789_000_000;
            assert_eq!(
                parse_reset("resets at 2026-09-17T00:00:00Z.", now),
                Some(1_789_603_200)
            );
        }

        // claim_2: Only the first occurrence of each reset prefix is examined,
        // so an unparseable first mention hides a valid later one.
        #[test]
        fn claim_2() {
            assert_eq!(
                parse_reset("retry-after: later, retry-after: 60", 1_000),
                Some(1_060)
            );
        }
    }
