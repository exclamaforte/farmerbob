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
