// Conformance suite for ux-json, derived from the task specification ALONE.
// Written without reading any candidate's implementation. Every assertion cites the
// spec clause it enforces; anything the spec does not fix is deliberately untested.
#[cfg(test)]
#[allow(unused_imports)]
mod conformance_ux_json {
    use crate::envelope::*;
    use serde_json::json;

    // "0 ok, 1 error, 2 usage, 3 would-block, 4 quota-limited."
    #[test]
    fn exit_codes_have_the_stated_numbers() {
        assert_eq!(ExitCode::Ok.code(), 0);
        assert_eq!(ExitCode::Error.code(), 1);
        assert_eq!(ExitCode::Usage.code(), 2);
        assert_eq!(ExitCode::WouldBlock.code(), 3);
        assert_eq!(ExitCode::QuotaLimited.code(), 4);
    }

    // "from_code returns None for an unmapped integer rather than guessing."
    #[test]
    fn from_code_is_none_for_unmapped_integers() {
        for c in [5, 9, -1, i32::MAX, i32::MIN] {
            assert!(ExitCode::from_code(c).is_none(), "code {c} should be unmapped");
        }
        for c in 0..=4 {
            assert_eq!(ExitCode::from_code(c).map(|e| e.code()), Some(c));
        }
    }

    // "to_line never emits an interior newline, even when a string field contains \n."
    #[test]
    fn to_line_is_one_line_even_with_embedded_newlines() {
        let e = Envelope::ok("t", 1, json!({"msg": "a\nb\nc", "nested": {"x": "\n"}}));
        let l = e.to_line().expect("serialises");
        assert_eq!(l.matches('\n').count(), 1, "exactly one newline, the terminator");
        assert!(l.ends_with('\n'), "terminated by a newline");
        assert!(!l[..l.len() - 1].contains('\n'), "no interior newline");
    }

    #[test]
    fn error_envelopes_are_also_one_line() {
        let e = Envelope::err("t", 1, "bad", "line one\nline two");
        let l = e.to_line().expect("serialises");
        assert_eq!(l.matches('\n').count(), 1);
        assert!(l.ends_with('\n'));
    }

    // "An error envelope's exit_code is Error unless its code is would_block or
    //  quota_limited, which map to those variants."
    #[test]
    fn error_code_strings_map_to_exit_codes() {
        assert_eq!(Envelope::err("t", 1, "would_block", "m").exit_code(), ExitCode::WouldBlock);
        assert_eq!(Envelope::err("t", 1, "quota_limited", "m").exit_code(), ExitCode::QuotaLimited);
        assert_eq!(Envelope::err("t", 1, "anything_else", "m").exit_code(), ExitCode::Error);
        assert_eq!(Envelope::err("t", 1, "", "m").exit_code(), ExitCode::Error);
        assert_eq!(Envelope::ok("t", 1, json!(null)).exit_code(), ExitCode::Ok);
    }

    // "Round-tripping an envelope through to_line and parse_line yields an equal value."
    #[test]
    fn round_trip_preserves_both_variants() {
        for e in [
            Envelope::ok("task.get", 1, json!({"id": 7, "tags": ["a", "b"]})),
            Envelope::ok("task.get", 2, json!(null)),
            Envelope::err("task.get", 1, "would_block", "a lease is held"),
        ] {
            let l = e.to_line().expect("serialises");
            let back = parse_line(&l).expect("parses");
            assert_eq!(back, e, "round trip must be lossless");
        }
    }

    // "parse_line tolerates unknown fields: a consumer built against version 1 has to
    //  keep working when version 2 adds a field."
    #[test]
    fn parse_line_tolerates_unknown_fields() {
        let ok = parse_line(r#"{"schema":"t","version":1,"data":{"a":1},"added_later":true}"#);
        assert!(ok.is_ok(), "unknown field must not be an error: {ok:?}");
        let er = parse_line(r#"{"schema":"t","version":1,"error":{"code":"c","message":"m"},"extra":9}"#);
        assert!(er.is_ok(), "unknown field must not be an error: {er:?}");
    }

    // "parse_line ... rejects a line that is not an envelope at all."
    #[test]
    fn parse_line_rejects_non_envelopes() {
        for bad in [
            "",
            "not json",
            "[]",
            "null",
            "42",
            r#"{"schema":"t"}"#,                       // no version, no body
            r#"{"version":1,"data":{}}"#,              // no schema
            r#"{"schema":"t","version":1}"#,           // neither data nor error
        ] {
            assert!(parse_line(bad).is_err(), "should reject: {bad:?}");
        }
    }

    // "schema() -> the schema name, whichever variant this is."
    #[test]
    fn schema_is_readable_from_either_variant() {
        assert_eq!(Envelope::ok("s.ok", 1, json!(1)).schema(), "s.ok");
        assert_eq!(Envelope::err("s.err", 1, "c", "m").schema(), "s.err");
    }

    // A terminated line is what to_line produces; parse_line must accept its own output.
    #[test]
    fn parse_line_accepts_a_trailing_newline() {
        let l = Envelope::ok("t", 1, json!({"k": "v"})).to_line().expect("serialises");
        assert!(parse_line(&l).is_ok(), "must parse its own output verbatim");
    }
}
