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
//!
//! Matching happens against [`normalise`]`ed` text, currently ANSI-stripped.
//! Launchers colourise, and a reset sequence between an anchored pattern's
//! prefix and its message makes the anchored form the one that misses (found
//! 2026-09-17, when thirteen OpenRouter refusals were each recorded as the
//! model producing nothing). The recognised patterns, exclusions and reset
//! formats remain known subsets, never closed sets: `Normal` claims only that
//! no known marker was present, not that a run was genuine.

/// What the harness concluded about one finished run, from its exit status and output.
///
/// The three conclusions are fixed, but the evidence they are drawn from is not:
/// the formats a provider uses to refuse service are a **known subset, not a
/// closed set**, and each new one has been discovered the same way — by finding
/// runs that had been scored as arm failures for some unknown period. The rules
/// in [`SignalRules`] are data precisely so extending that subset never requires
/// touching this type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classification {
    /// No refusal marker was recognised in the output.
    ///
    /// This is deliberately weaker than "confirmed to be a genuine run". It
    /// means only that none of the markers this rule set knows were present;
    /// a refusal phrased in a form no pattern catches also lands here. That
    /// gap is how the 2026-09-17 incident recorded refused arms as NO-OPs.
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
        /// The matched text from the ANSI-normalised output (see [`normalise`]),
        /// for a human deciding whether the classifier is right. Never carries
        /// escape codes: a record that does cannot be compared against a
        /// pattern by anyone reading it later.
        evidence: String,
    },
    /// Something matched a limit-shaped pattern but the rule set is not confident.
    /// The caller must not park a bucket on this alone.
    Ambiguous {
        /// The matched text from the ANSI-normalised output (see [`normalise`]),
        /// for a human deciding whether the classifier is right. Never carries
        /// escape codes.
        evidence: String,
    },
}

/// One provider's recognition rules. Data, not code: a new adapter is a new value,
/// never a new match arm.
///
/// The pattern lists below are **known subsets, not closed sets**: the ways a
/// provider can say no are open-ended and grow. Extend a list only when a
/// mis-scored run proves the gap — never speculatively, since a speculatively
/// broad pattern is how an agent *discussing* quotas gets misread as one being
/// refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalRules {
    /// Exit codes that mean quota, e.g. `vec![429]`.
    pub limit_exit_codes: Vec<i32>,
    /// Case-insensitive substrings of stderr/stdout that mean quota. A known
    /// subset of refusal formats, not a closed set. Matched against
    /// [`normalise`]d output, so anchored patterns survive launcher colour.
    pub limit_patterns: Vec<String>,
    /// Substrings that look like a limit but are NOT one, checked first.
    /// e.g. "rate limit" inside a model's own explanatory prose. Also a known
    /// subset; also matched against [`normalise`]d output.
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
/// Patterns and exclusions are matched against `normalise(output)`, not
/// the raw bytes: a launcher that colourises its refusal puts escape sequences
/// between an anchored pattern's prefix and its message, and matching raw bytes
/// silently defeats anchoring (the 2026-09-17 incident). The [`evidence`]
/// carried by [`Classification::Limited`] and [`Classification::Ambiguous`] is a
/// slice of that same normalised text, so records never contain escape codes.
///
/// The decision order is: exclusions first (any hit means [`Classification::Normal`],
/// even when a limit pattern also matched), then limit patterns (a hit with exit
/// code `0` is [`Classification::Ambiguous`], otherwise [`Classification::Limited`]),
/// then limit exit codes (a hit alone suffices for [`Classification::Limited`]).
/// Anything else is [`Classification::Normal`]: ordinary failure is the common
/// case and must not pollute the quota path.
///
/// [`evidence`]: Classification::Limited::evidence
pub fn classify(rules: &SignalRules, exit_code: i32, output: &str, now: u64) -> Classification {
    let text = normalise(output);
    if rules
        .exclusions
        .iter()
        .any(|exclusion| find_insensitive(&text, exclusion).is_some())
    {
        return Classification::Normal;
    }
    if let Some(evidence) = first_match_text(&text, &rules.limit_patterns) {
        if exit_code == 0 {
            return Classification::Ambiguous { evidence };
        }
        return Classification::Limited {
            reset_at: parse_reset(&text, now),
            evidence,
        };
    }
    if rules.limit_exit_codes.contains(&exit_code) {
        // The fallback keys on the normalised text: an output of escape codes
        // alone leaves a human nothing to read, exactly like no output at all.
        let reset_at = parse_reset(&text, now);
        let evidence = if text.is_empty() {
            format!("exit code {exit_code}")
        } else {
            text
        };
        return Classification::Limited { reset_at, evidence };
    }
    Classification::Normal
}

/// Strip ANSI escape sequences from `s`, returning owned text.
///
/// Removes CSI sequences (`ESC [` … final byte in `0x40..=0x7E`) and two-character
/// `ESC <byte>` sequences. Text containing no escapes is returned unchanged.
///
/// A truncated escape at end-of-input (`"abc\x1b"`, `"abc\x1b["`) is dropped
/// without panicking or looping: logs get cut off mid-sequence.
///
/// ```
/// use farmerbob_core::limit_signal::strip_ansi;
/// assert_eq!(strip_ansi("\u{1b}[91m\u{1b}[1mError: \u{1b}[0mRate limit exceeded"), "Error: Rate limit exceeded");
/// assert_eq!(strip_ansi("no escapes here"), "no escapes here");
/// ```
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        // CSI: consume through the final byte (0x40..=0x7E). Any other byte
        // after ESC begins a two-character sequence; drop that byte too.
        // Either scan ends harmlessly at end-of-input.
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            // OSC, DCS, APC, PM, SOS: a string-carrying introducer whose PAYLOAD must go
            // too, not just the two opening bytes.
            //
            // Dropping only `ESC ]` left the payload in the text. A critique on
            // refusal-norm demonstrated it against the merged code:
            //
            //     strip_ansi("\x1b]8;;https://x/rate\x1b\\ ok \x1b]8;;\x1b\\")
            //         == "8;;https://x/rate ok 8;;"
            //
            // and, worse, a launcher setting a terminal title made classify() return
            // Limited for a build that SUCCEEDED:
            //
            //     classify(rules, 1, "\x1b]0;rate limit exceeded\x1b\\ build succeeded")
            //         == Limited { evidence: "rate limit exceeded" }
            //
            // That is the morning's bug wearing a different escape family: text that is not
            // the launcher refusing, read as the launcher refusing. Here it costs an arm
            // credit for work it actually did, because a quota_limited run is excluded from
            // arm results entirely.
            //
            // These sequences terminate at BEL (0x07) or at ST (`ESC \`). An unterminated
            // one runs to end-of-input, which is the correct reading: everything after an
            // unterminated introducer IS its payload.
            Some(']' | 'P' | '_' | '^' | 'X') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' {
                        // ST is `ESC \`; any other ESC ends this string too and begins
                        // the next sequence, which the outer loop would have to re-read.
                        // Consuming one byte here is right for ST and harmless otherwise.
                        let _ = chars.next();
                        break;
                    }
                }
            }
            Some(_) | None => {}
        }
    }
    out
}

/// The text [`classify`] actually matches against, exposed so a caller can see what was searched.
///
/// Currently: ANSI-stripped. Casing is NOT folded here — matching is already
/// case-insensitive and folding twice would be a second place to get it wrong.
///
/// ```
/// use farmerbob_core::limit_signal::normalise;
/// assert_eq!(normalise("\u{1b}[0mRate limit exceeded"), "Rate limit exceeded");
/// assert_eq!(normalise("MiXeD case"), "MiXeD case");
/// ```
pub fn normalise(s: &str) -> String {
    strip_ansi(s)
}

/// Extract an absolute reset instant from free text, if one is stated.
///
/// Recognises, at minimum (a **known subset, not a closed set** — more formats
/// exist and more will appear):
///   - `retry-after: 3600`              (seconds from `now`)
///   - `"reset_at": 1789000000`         (absolute unix seconds)
///   - `resets at 2026-09-17T00:00:00Z` (RFC 3339)
///
/// Returns [`None`] rather than guessing on unparseable input. A *relative*
/// statement never resolves to an instant in the past relative to `now`.
///
/// The text is matched after [`normalise`] strips escape sequences. The reset
/// statement travels in the same launcher stream as the refusal line, so it had
/// exactly the same colour exposure: the OpenRouter launcher wraps its whole
/// error line, reset statement included, and reading raw bytes would miss a
/// `retry-after:` split by a reset sequence just as anchoring was missed.
pub fn parse_reset(output: &str, now: u64) -> Option<u64> {
    let text = normalise(output);
    parse_reset_at_field(&text)
        .or_else(|| parse_resets_at_timestamp(&text))
        .or_else(|| parse_relative_seconds(&text, now))
}

// Returns the exact text slice matched by the first matching pattern, taken
// from the (normalised) text passed in, preserving its original casing for
// human review.
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
            match (
                hay_chars.get(start_index + offset),
                needle_chars.get(offset),
            ) {
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
// out of the past even for `0` or saturating inputs. The keyword list is a
// known subset of the phrasings that introduce a relative reset, not a closed
// set; see `parse_reset`.
/// A duration written after a retry keyword, in seconds.
///
/// UNITS ARE NOT OPTIONAL TO READ. This used to take the leading digits and call them
/// seconds, which is right for an HTTP `Retry-After: 120` and wrong for every duration a
/// provider writes for a human. agy says `Individual quota reached ... Resets in 3h46m52s.`
/// and that parsed as THREE SECONDS. So a park computed from it expired the moment it was
/// written, and the longer the refusal the more completely the park did nothing -- exactly
/// inverted. Both agy arms were refused on 2026-09-19 with hour-scale resets, and the park
/// this would have applied was under four seconds.  (bead farmerbob-4uha)
///
/// A bare number keeps its old meaning, seconds, so `Retry-After` is unaffected. A number
/// followed by a unit is scaled, and adjacent `<number><unit>` pairs accumulate, because
/// `3h46m52s` is one duration written in three parts.
fn parse_duration_secs(tail: &str) -> Option<u64> {
    let mut rest = tail;
    let mut total: u64 = 0;
    let mut saw_any = false;

    loop {
        let digits = leading_digits(rest);
        if digits.is_empty() {
            break;
        }
        let value: u64 = digits.parse().ok()?;
        rest = rest.get(digits.len()..).unwrap_or("");
        // An optional space between the number and its unit: "5 minutes".
        let after_space = skip_while(rest, |c| c == ' ');
        let (multiplier, consumed) = unit_at(after_space);
        total = total.saturating_add(value.saturating_mul(multiplier));
        saw_any = true;
        if consumed == 0 {
            // No unit: a bare number is seconds, and nothing can follow it.
            break;
        }
        rest = after_space.get(consumed..).unwrap_or("");
    }

    saw_any.then_some(total)
}

/// The unit at the head of `s`: its multiplier in seconds, and how many bytes it occupies.
///
/// `(1, 0)` means no unit was found, so the caller treats the number as bare seconds.
/// Longest spellings are tried first, or `m` would match the start of `minutes`.
fn unit_at(s: &str) -> (u64, usize) {
    const UNITS: &[(&str, u64)] = &[
        ("hours", 3600),
        ("hour", 3600),
        ("hrs", 3600),
        ("hr", 3600),
        ("h", 3600),
        ("minutes", 60),
        ("minute", 60),
        ("mins", 60),
        ("min", 60),
        ("m", 60),
        ("seconds", 1),
        ("second", 1),
        ("secs", 1),
        ("sec", 1),
        ("s", 1),
    ];
    for (name, mult) in UNITS {
        if s.len() >= name.len()
            && s.get(..name.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(name))
        {
            return (*mult, name.len());
        }
    }
    (1, 0)
}

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
            match parse_duration_secs(tail) {
                Some(seconds) => return Some(now.saturating_add(seconds)),
                None => continue,
            }
        }
    }
    None
}

// Absolute unix seconds ("reset_at": 1789000000). Returned as stated, even when
// in the past: only relative statements are clamped to the present. Keyword
// list is a known subset, not a closed set.
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

// Prefixed RFC 3339 statements ("resets at 2026-09-17T00:00:00Z"). Keyword
// list is a known subset, not a closed set.
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
        assert_eq!(
            classify(&rules, 0, "all work finished", 1000),
            Classification::Normal
        );
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
        assert_eq!(
            parse_reset("Error 429: retry-after: 3600, slow down", 1000),
            Some(4600)
        );
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
            parse_reset(
                "usage limit hit, resets at 2026-09-17T00:00:00Z please wait",
                1000
            ),
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
        assert_eq!(
            parse_reset("retry-after: 99999999999999999999999999", 1000),
            None
        );
    }

    #[test]
    fn empty_rules_never_signal() {
        let rules = SignalRules::new(60);
        assert_eq!(
            classify(&rules, 1, "rate limit quota exceeded 429", 1000),
            Classification::Normal
        );
        assert_eq!(
            classify(&rules, 429, "rate limit", 1000),
            Classification::Normal
        );
    }

    #[test]
    fn empty_pattern_and_exclusion_are_inert() {
        let rules = SignalRules::new(60).with_pattern("").with_exclusion("");
        assert_eq!(
            classify(&rules, 1, "anything at all", 1000),
            Classification::Normal
        );
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
        assert_eq!(
            parse_reset(r#""RESET_AT": 1789000000"#, 1000),
            Some(1789000000)
        );
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
            classify(
                &rules(),
                1,
                "thread 'main' panicked at src/main.rs:4",
                1_000
            ),
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
        for junk in [
            "",
            "no reset here",
            "later",
            "soon-ish",
            "retry-after: eventually",
        ] {
            assert_eq!(
                parse_reset(junk, 1_000),
                None,
                "must not guess from {junk:?}"
            );
        }
    }

    // "... and never returns an instant in the past relative to now for a relative statement."
    #[test]
    fn a_relative_reset_is_never_in_the_past() {
        if let Some(t) = parse_reset("retry-after: 60", 5_000) {
            assert!(
                t >= 5_000,
                "a relative reset must not resolve into the past"
            );
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
        assert_eq!(
            classify(&r, 1, "please do not slow down", 0),
            Classification::Normal
        );
    }
}

// Clause tests for colour and casing survival, derived from the 2026-09-17
// incident: thirteen OpenRouter refusals, each recorded as the model producing
// nothing, because the launcher's reset sequence sat between the `Error: `
// prefix and the message and the anchored pattern never matched raw bytes.
#[cfg(test)]
mod colour_survival {
    use super::*;

    // The verbatim bytes from the incident log. Note there is NO contiguous
    // `Error: Rate limit exceeded` in here: `\x1b[0m` sits between them.
    const INCIDENT_LINE: &str =
        "\u{1b}[91m\u{1b}[1mError: \u{1b}[0mRate limit exceeded: free-models-per-day-high-balance.";

    // Clause 1: strip_ansi removes exactly the incident's escapes.
    #[test]
    fn strip_ansi_removes_the_incident_escapes() {
        assert_eq!(
            strip_ansi("\u{1b}[91m\u{1b}[1mError: \u{1b}[0mRate limit exceeded"),
            "Error: Rate limit exceeded"
        );
    }

    // Clause 2: escape-free text is returned unchanged, byte for byte.
    #[test]
    fn strip_ansi_is_identity_without_escapes() {
        for s in [
            "",
            "plain ascii text",
            "Error: Rate limit exceeded: free-models-per-day-high-balance.",
            "unicode héllo — ✓, tabs\tnewlines\nand\rcarriage returns",
            "brackets and codes that only LOOK like escapes: [91m [0m",
        ] {
            assert_eq!(strip_ansi(s).as_bytes(), s.as_bytes());
            assert_eq!(strip_ansi(s), s);
        }
    }

    #[test]
    fn strip_ansi_removes_csi_sequences_with_parameters() {
        assert_eq!(strip_ansi("\u{1b}[38;5;196mred\u{1b}[49m\u{1b}[0m"), "red");
        assert_eq!(strip_ansi("a\u{1b}[1mb\u{1b}[22m c"), "ab c");
    }

    #[test]
    fn strip_ansi_removes_two_character_escapes() {
        assert_eq!(strip_ansi("\u{1b}Mline one"), "line one");
        assert_eq!(strip_ansi("before\u{1b}cafter"), "beforeafter");
    }

    // Boundary: escape-only input strips to nothing.
    #[test]
    fn strip_ansi_escape_only_input_is_empty() {
        assert_eq!(strip_ansi("\u{1b}[0m"), "");
        assert_eq!(strip_ansi("\u{1b}M"), "");
    }

    // Boundary: a truncated escape at end-of-input must neither panic nor loop.
    #[test]
    fn strip_ansi_truncated_escape_at_end_of_input() {
        assert_eq!(strip_ansi("abc\u{1b}"), "abc");
        assert_eq!(strip_ansi("abc\u{1b}["), "abc");
        assert_eq!(strip_ansi("abc\u{1b}[91"), "abc");
        assert_eq!(strip_ansi("\u{1b}"), "");
        assert_eq!(strip_ansi("\u{1b}["), "");
    }

    // The pinned definition of `normalise`: ANSI-stripped, casing NOT folded.
    #[test]
    fn normalise_strips_escapes_but_not_case() {
        assert_eq!(
            normalise("\u{1b}[91m\u{1b}[1mError: \u{1b}[0mRate limit exceeded"),
            "Error: Rate limit exceeded"
        );
        assert_eq!(normalise("MiXeD Case"), "MiXeD Case");
        assert_eq!(normalise("no escapes"), "no escapes");
    }

    // Clause 3: THE regression. Against these bytes the anchored pattern read
    // as Normal before the fix — colour broke the byte contiguity of
    // `Error: Rate limit exceeded` that anchoring depends on.
    #[test]
    fn anchored_pattern_survives_colourised_prefix() {
        let rules = SignalRules::new(60).with_pattern("error: rate limit exceeded");
        match classify(&rules, 1, INCIDENT_LINE, 1_000) {
            Classification::Limited { reset_at, evidence } => {
                assert_eq!(evidence, "Error: Rate limit exceeded");
                assert_eq!(reset_at, None);
            }
            other => panic!("colourised anchored refusal must be Limited, got {other:?}"),
        }
    }

    // Clause 4: the fix must not trade the anchored form for the unanchored
    // one — both shapes must fire against the same bytes.
    #[test]
    fn unanchored_pattern_still_matches_colourised_text() {
        let rules = SignalRules::new(60).with_pattern("rate limit exceeded");
        match classify(&rules, 1, INCIDENT_LINE, 1_000) {
            Classification::Limited { .. } => {}
            other => panic!("colourised unanchored refusal must be Limited, got {other:?}"),
        }
    }

    // Colour can land anywhere, not only at the prefix: an escape INSIDE the
    // phrase a pattern anchors on must not break the match either.
    #[test]
    fn escape_inside_the_phrase_does_not_break_the_match() {
        let rules = SignalRules::new(60).with_pattern("rate limit exceeded");
        match classify(&rules, 1, "rate \u{1b}[0mlimit exceeded now", 1_000) {
            Classification::Limited { evidence, .. } => {
                assert_eq!(evidence, "rate limit exceeded");
            }
            other => panic!("expected Limited, got {other:?}"),
        }
    }

    // Clause 5: exclusions are matched against normalised text too. The reset
    // sequence here splits the exclusion phrase itself, so only normalised
    // matching can see it.
    #[test]
    fn exclusion_fires_on_colourised_text() {
        let rules = SignalRules::new(60)
            .with_pattern("rate limit")
            .with_exclusion("as an example of rate limit");
        let colourised = "\u{1b}[1mas an example \u{1b}[0mof rate limit output";
        assert_eq!(
            classify(&rules, 1, colourised, 1_000),
            Classification::Normal
        );
    }

    // Clause 6: parse_reset reads normalised text — the reset statement
    // travels in the same colourised launcher stream as the refusal line.
    #[test]
    fn parse_reset_finds_a_reset_in_colourised_text() {
        let colourised = "\u{1b}[91mError: \u{1b}[0mRate limit exceeded; retry-after: 3600";
        assert_eq!(parse_reset(colourised, 1_000), Some(4_600));
        let split_mid_phrase = "resets \u{1b}[0mat 2026-09-17T00:00:00Z";
        assert_eq!(parse_reset(split_mid_phrase, 1_000), Some(1_789_603_200));
    }

    #[test]
    fn classify_recovers_the_reset_from_colourised_text() {
        let rules = SignalRules::new(60).with_pattern("rate limit exceeded");
        let colourised = "\u{1b}[91mError: \u{1b}[0mRate limit exceeded; retry-after: 3600";
        match classify(&rules, 1, colourised, 1_000) {
            Classification::Limited { reset_at, .. } => assert_eq!(reset_at, Some(4_600)),
            other => panic!("expected Limited, got {other:?}"),
        }
    }

    // Clause 7: evidence on a Limited from colourised input contains no \x1b —
    // a record that still carries escape codes cannot be compared against a
    // pattern by anyone reading it later. The same holds on every path that
    // fills evidence.
    #[test]
    fn limited_evidence_is_normalised_not_raw() {
        let rules = SignalRules::new(60).with_pattern("error: rate limit exceeded");
        match classify(&rules, 1, INCIDENT_LINE, 1_000) {
            Classification::Limited { evidence, .. } => {
                assert!(
                    !evidence.contains('\u{1b}'),
                    "raw evidence leaked: {evidence:?}"
                );
                assert_eq!(evidence, "Error: Rate limit exceeded");
            }
            other => panic!("expected Limited, got {other:?}"),
        }
    }

    #[test]
    fn ambiguous_evidence_is_normalised_not_raw() {
        let rules = SignalRules::new(60).with_pattern("rate limit exceeded");
        match classify(&rules, 0, INCIDENT_LINE, 1_000) {
            Classification::Ambiguous { evidence } => {
                assert!(
                    !evidence.contains('\u{1b}'),
                    "raw evidence leaked: {evidence:?}"
                );
                assert_eq!(evidence, "Rate limit exceeded");
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn exit_code_evidence_is_normalised_not_raw() {
        let rules = SignalRules::new(60).with_exit_code(429);
        match classify(
            &rules,
            429,
            "\u{1b}[91mError\u{1b}[0m: spend cap reached",
            1_000,
        ) {
            Classification::Limited { evidence, .. } => {
                assert!(
                    !evidence.contains('\u{1b}'),
                    "raw evidence leaked: {evidence:?}"
                );
                assert_eq!(evidence, "Error: spend cap reached");
            }
            other => panic!("expected Limited, got {other:?}"),
        }
    }

    // Boundary: empty output with a registered limit exit code keeps the
    // exit-code evidence and stays Limited.
    #[test]
    fn empty_output_with_limit_exit_code_keeps_exit_code_evidence() {
        let rules = SignalRules::new(60).with_exit_code(1);
        match classify(&rules, 1, "", 1_000) {
            Classification::Limited { evidence, reset_at } => {
                assert_eq!(evidence, "exit code 1");
                assert_eq!(reset_at, None);
            }
            other => panic!("expected Limited, got {other:?}"),
        }
    }

    // Boundary: zero patterns and zero exit codes must classify everything
    // Normal — never Limited, never a panic — whatever the input.
    #[test]
    fn zero_rules_classify_normal_and_never_panic() {
        let rules = SignalRules::new(60);
        let cases: [(i32, &str); 6] = [
            (1, ""),
            (1, INCIDENT_LINE),
            (1, "rate limit exceeded, quota gone, 429"),
            (0, "normal completion"),
            (429, "\u{1b}[0m"),
            (-1, "\u{1b}"),
        ];
        for (code, out) in cases {
            assert_eq!(
                classify(&rules, code, out, 1_000),
                Classification::Normal,
                "empty rules must be inert for exit {code}, output {out:?}"
            );
        }
    }

    // Pins first_match_text's existing choice, so it cannot change by
    // accident: patterns are tried in LIST order first; only then does
    // leftmost position matter, and only within one pattern.
    #[test]
    fn several_patterns_match_in_list_order_not_text_position() {
        let rules = SignalRules::new(60)
            .with_pattern("quota exceeded")
            .with_pattern("rate limit");
        match classify(&rules, 1, "rate limit first, then quota exceeded", 1_000) {
            Classification::Limited { evidence, .. } => {
                assert_eq!(evidence, "quota exceeded");
            }
            other => panic!("expected Limited, got {other:?}"),
        }
    }

    #[test]
    fn one_pattern_matches_its_leftmost_occurrence() {
        let rules = SignalRules::new(60).with_pattern("limit");
        match classify(&rules, 1, "rate limit; another limit here", 1_000) {
            Classification::Limited { evidence, .. } => {
                assert_eq!(evidence, "limit");
            }
            other => panic!("expected Limited, got {other:?}"),
        }
    }

    // The pattern list order pin holds under colour too, and the evidence for
    // the list-first pattern is read from the normalised text.
    #[test]
    fn list_order_pin_survives_colour() {
        let rules = SignalRules::new(60)
            .with_pattern("quota exceeded")
            .with_pattern("error: rate limit");
        let both_present = INCIDENT_LINE.replace("Rate limit", "Rate limit (quota exceeded)");
        match classify(&rules, 1, &both_present, 1_000) {
            Classification::Limited { evidence, .. } => {
                assert_eq!(evidence, "quota exceeded");
            }
            other => panic!("expected Limited, got {other:?}"),
        }
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

#[cfg(test)]
mod osc_regression {
    use super::*;

    /// Found by a critique on refusal-norm, against code already merged. An OSC payload
    /// survived stripping, so text that is not the launcher refusing could read as one.
    #[test]
    fn an_osc_payload_does_not_survive_stripping() {
        let hyperlink = "\u{1b}]8;;https://x/rate\u{1b}\\ ok \u{1b}]8;;\u{1b}\\";
        assert_eq!(strip_ansi(hyperlink).trim(), "ok");
    }

    /// The consequence, and the reason this is a defect rather than untidiness: a launcher
    /// setting a terminal title made a SUCCESSFUL build classify as Limited, which excludes
    /// the run from arm results and costs the arm credit for work it did.
    #[test]
    fn a_terminal_title_cannot_manufacture_a_refusal() {
        let rules = SignalRules::new(3600).with_pattern("rate limit exceeded");
        let titled = "\u{1b}]0;rate limit exceeded\u{1b}\\ build succeeded";
        assert!(
            matches!(classify(&rules, 1, titled, 0), Classification::Normal),
            "an OSC title is not the launcher refusing"
        );
    }

    /// And the real refusal must still fire. Both directions in one place so neither can be
    /// fixed by breaking the other.
    #[test]
    fn a_real_colourised_refusal_still_fires() {
        let rules = SignalRules::new(3600).with_pattern("error: rate limit exceeded");
        let real = "\u{1b}[91m\u{1b}[1mError: \u{1b}[0mRate limit exceeded: free-models-per-day.";
        assert!(matches!(
            classify(&rules, 1, real, 0),
            Classification::Limited { .. }
        ));
    }

    /// A BEL-terminated OSC, the other terminator.
    #[test]
    fn a_bel_terminated_osc_is_stripped() {
        assert_eq!(strip_ansi("\u{1b}]0;title\u{7}after").trim(), "after");
    }

    /// An unterminated introducer runs to end of input: everything after it IS its payload,
    /// and keeping that text is how the false positive got in.
    #[test]
    fn an_unterminated_osc_consumes_the_rest() {
        assert_eq!(strip_ansi("keep\u{1b}]0;never closed"), "keep");
    }

    /// UNITS ARE NOT OPTIONAL TO READ. The leading digits used to be taken as seconds, which
    /// is right for `Retry-After: 120` and wrong for every duration a provider writes for a
    /// human. agy's real refusal -- "Resets in 3h46m52s" -- parsed as THREE SECONDS, so the
    /// park expired the moment it was written, and the LONGER the refusal the more
    /// completely the park did nothing.  (bead farmerbob-4uha)
    #[test]
    fn a_compound_duration_is_read_whole() {
        let text = "error: Individual quota reached. Please upgrade your subscription to \
                    increase your limits. Resets in 3h46m52s.";
        assert_eq!(
            parse_reset(text, 1_000),
            Some(1_000 + 3 * 3600 + 46 * 60 + 52),
            "3h46m52s is 13612 seconds, not 3"
        );
    }

    /// A BARE NUMBER KEEPS ITS OLD MEANING. HTTP states `Retry-After` in seconds with no
    /// unit, and that must not change.
    #[test]
    fn a_bare_number_is_still_seconds() {
        assert_eq!(parse_reset("retry-after: 120", 1_000), Some(1_120));
    }

    /// Spelled-out units, with and without a space.
    #[test]
    fn spelled_units_are_read() {
        assert_eq!(parse_reset("try again in 5 minutes", 0), Some(300));
        assert_eq!(parse_reset("retry in 2 hours", 0), Some(7_200));
        assert_eq!(parse_reset("resets in 90s", 0), Some(90));
        assert_eq!(parse_reset("resets in 1h30m", 0), Some(5_400));
    }

    /// `m` must not swallow the start of `minutes`, and the longest spelling wins.
    #[test]
    fn a_unit_prefix_does_not_shadow_a_longer_spelling() {
        assert_eq!(parse_reset("retry in 1 minute", 0), Some(60));
        assert_eq!(parse_reset("retry in 1 min", 0), Some(60));
        assert_eq!(parse_reset("retry in 1m", 0), Some(60));
    }

    /// A keyword with no number after it yields nothing, rather than a park of zero. A park
    /// that expires immediately is indistinguishable from no park at all.
    #[test]
    fn a_keyword_with_no_duration_yields_nothing() {
        assert_eq!(parse_reset("resets in a little while", 1_000), None);
    }
}
