//! Versioned JSON output envelope for machine-readable command output.
//!
//! The planning agent consumes one JSON record per line, so every command
//! emits exactly one [`Envelope`] serialised with [`Envelope::to_line`].

/// Branchable outcome. The planning agent switches on this without reading prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExitCode {
    /// Command succeeded.
    Ok,
    /// Command failed with a generic error.
    Error,
    /// Command was invoked incorrectly.
    Usage,
    /// Command could not proceed without blocking.
    WouldBlock,
    /// Command was refused because a quota was exhausted.
    QuotaLimited,
}

impl ExitCode {
    /// 0 ok, 1 error, 2 usage, 3 would-block, 4 quota-limited.
    pub fn code(self) -> i32 {
        match self {
            ExitCode::Ok => 0,
            ExitCode::Error => 1,
            ExitCode::Usage => 2,
            ExitCode::WouldBlock => 3,
            ExitCode::QuotaLimited => 4,
        }
    }

    /// Decode an exit code integer back to an [`ExitCode`].
    ///
    /// Returns `None` for an unmapped integer rather than guessing.
    pub fn from_code(code: i32) -> Option<ExitCode> {
        match code {
            0 => Some(ExitCode::Ok),
            1 => Some(ExitCode::Error),
            2 => Some(ExitCode::Usage),
            3 => Some(ExitCode::WouldBlock),
            4 => Some(ExitCode::QuotaLimited),
            _ => None,
        }
    }
}

/// Machine-readable error payload carried by [`Envelope::Err`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ErrorBody {
    /// Stable machine-readable error identifier.
    pub code: String,
    /// Human-readable explanation of the failure.
    pub message: String,
}

/// Every command emits exactly this shape under --json.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum Envelope {
    /// Successful command output.
    Ok {
        /// Name of the schema this payload conforms to.
        schema: String,
        /// Version of the named schema.
        version: u32,
        /// Arbitrary payload for the named schema.
        data: serde_json::Value,
    },
    /// Failed command output, mirroring the success shape.
    Err {
        /// Name of the schema this payload conforms to.
        schema: String,
        /// Version of the named schema.
        version: u32,
        /// Machine-readable error payload.
        error: ErrorBody,
    },
}

impl Envelope {
    /// Build a success envelope for the given schema and payload.
    pub fn ok(schema: &str, version: u32, data: serde_json::Value) -> Envelope {
        Envelope::Ok {
            schema: schema.to_owned(),
            version,
            data,
        }
    }

    /// Build an error envelope for the given schema, code, and message.
    pub fn err(schema: &str, version: u32, code: &str, message: &str) -> Envelope {
        Envelope::Err {
            schema: schema.to_owned(),
            version,
            error: ErrorBody {
                code: code.to_owned(),
                message: message.to_owned(),
            },
        }
    }

    /// Serialise to exactly ONE line: no interior newline, terminated by `\n`.
    /// A multi-line record breaks any consumer reading line by line.
    pub fn to_line(&self) -> Result<String, String> {
        match serde_json::to_string(self) {
            Ok(mut line) => {
                line.push('\n');
                Ok(line)
            }
            Err(e) => Err(e.to_string()),
        }
    }

    /// Branchable exit status for this envelope.
    ///
    /// Success is [`ExitCode::Ok`]. An error envelope maps to
    /// [`ExitCode::Error`], unless its error code identifies a
    /// would-block or quota-limited condition.
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Envelope::Ok { .. } => ExitCode::Ok,
            Envelope::Err { error, .. } => match error.code.as_str() {
                "would_block" => ExitCode::WouldBlock,
                "quota_limited" => ExitCode::QuotaLimited,
                _ => ExitCode::Error,
            },
        }
    }

    /// The schema name, whichever variant this is.
    pub fn schema(&self) -> &str {
        match self {
            Envelope::Ok { schema, .. } => schema.as_str(),
            Envelope::Err { schema, .. } => schema.as_str(),
        }
    }
}

/// Parse a line back. Unknown fields must NOT be an error: a consumer built against version 1
/// has to keep working when version 2 adds a field.
pub fn parse_line(line: &str) -> Result<Envelope, String> {
    match serde_json::from_str(line) {
        Ok(envelope) => Ok(envelope),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn to_line_has_no_interior_newline_when_data_contains_newline() {
        let env = Envelope::ok("demo", 1, json!({"text": "a\nb\nc"}));
        let line = env.to_line().expect("serialise");
        assert!(line.ends_with('\n'), "line must be newline terminated");
        let body = line.strip_suffix('\n').expect("terminator");
        assert!(!body.contains('\n'), "no interior newline allowed");
        // The escaped payload must still decode to the original value.
        let back = parse_line(&line).expect("parse back");
        assert_eq!(back, env);
    }

    #[test]
    fn to_line_has_no_interior_newline_when_error_message_contains_newline() {
        let env = Envelope::err("demo", 1, "boom", "line one\nline two");
        let line = env.to_line().expect("serialise");
        assert!(line.ends_with('\n'));
        let body = line.strip_suffix('\n').expect("terminator");
        assert!(!body.contains('\n'));
        assert_eq!(parse_line(&line).expect("parse back"), env);
    }

    #[test]
    fn round_trip_equality_for_ok_variant() {
        let env = Envelope::ok("task", 1, json!({"n": 42, "tags": ["a", "b"]}));
        let line = env.to_line().expect("serialise");
        assert_eq!(parse_line(&line).expect("parse"), env);
    }

    #[test]
    fn round_trip_equality_for_err_variant() {
        let env = Envelope::err("task", 2, "boom", "it broke");
        let line = env.to_line().expect("serialise");
        assert_eq!(parse_line(&line).expect("parse"), env);
    }

    #[test]
    fn parse_line_tolerates_unknown_fields() {
        let raw = r#"{"schema":"task","version":1,"data":{"n":1},"extra_future_field":123}"#;
        let parsed = parse_line(raw).expect("unknown fields must parse");
        assert_eq!(parsed.schema(), "task");
        assert_eq!(
            parsed,
            Envelope::ok("task", 1, json!({"n": 1})),
            "unknown field must be ignored"
        );
        let raw_err =
            r#"{"schema":"task","version":1,"error":{"code":"boom","message":"x"},"another":true}"#;
        let parsed_err = parse_line(raw_err).expect("unknown fields must parse");
        assert_eq!(parsed_err, Envelope::err("task", 1, "boom", "x"));
    }

    #[test]
    fn parse_line_rejects_a_line_that_is_not_an_envelope() {
        assert!(parse_line("not json at all").is_err());
        assert!(parse_line("[1,2,3]").is_err());
        assert!(parse_line(r#"{"schema":"task"}"#).is_err());
        assert!(parse_line("").is_err());
    }

    #[test]
    fn from_code_returns_none_for_unmapped_integer() {
        assert_eq!(ExitCode::from_code(9), None);
        assert_eq!(ExitCode::from_code(-1), None);
        assert_eq!(ExitCode::from_code(100), None);
    }

    #[test]
    fn from_code_round_trips_mapped_codes() {
        for (code, variant) in [
            (0, ExitCode::Ok),
            (1, ExitCode::Error),
            (2, ExitCode::Usage),
            (3, ExitCode::WouldBlock),
            (4, ExitCode::QuotaLimited),
        ] {
            assert_eq!(ExitCode::from_code(code), Some(variant));
            assert_eq!(variant.code(), code);
        }
    }

    #[test]
    fn error_code_would_block_maps_to_exit_code_three() {
        let env = Envelope::err("lease", 1, "would_block", "busy");
        assert_eq!(env.exit_code(), ExitCode::WouldBlock);
        assert_eq!(env.exit_code().code(), 3);
    }

    #[test]
    fn error_code_quota_limited_maps_to_exit_code_four() {
        let env = Envelope::err("lease", 1, "quota_limited", "empty");
        assert_eq!(env.exit_code(), ExitCode::QuotaLimited);
        assert_eq!(env.exit_code().code(), 4);
    }

    #[test]
    fn error_code_other_maps_to_generic_error() {
        let env = Envelope::err("lease", 1, "boom", "broke");
        assert_eq!(env.exit_code(), ExitCode::Error);
        assert_eq!(env.exit_code().code(), 1);
        let ok = Envelope::ok("lease", 1, json!(null));
        assert_eq!(ok.exit_code(), ExitCode::Ok);
        assert_eq!(ok.exit_code().code(), 0);
    }

    #[test]
    fn schema_returns_name_for_both_variants() {
        assert_eq!(Envelope::ok("a", 1, json!(null)).schema(), "a");
        assert_eq!(Envelope::err("b", 1, "c", "d").schema(), "b");
    }
}
// Conformance suite for ux-json, derived from the task specification ALONE.
// Written without reading any candidate's implementation. Every assertion cites the
// spec clause it enforces; anything the spec does not fix is deliberately untested.
#[cfg(test)]
#[allow(unused_imports)]
mod conformance_ux_json {
    // frozen suite, merged from .fb/conformance/ux-json.rs
    use super::*;
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
            assert!(
                ExitCode::from_code(c).is_none(),
                "code {c} should be unmapped"
            );
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
        assert_eq!(
            l.matches('\n').count(),
            1,
            "exactly one newline, the terminator"
        );
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
        assert_eq!(
            Envelope::err("t", 1, "would_block", "m").exit_code(),
            ExitCode::WouldBlock
        );
        assert_eq!(
            Envelope::err("t", 1, "quota_limited", "m").exit_code(),
            ExitCode::QuotaLimited
        );
        assert_eq!(
            Envelope::err("t", 1, "anything_else", "m").exit_code(),
            ExitCode::Error
        );
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
        let er = parse_line(
            r#"{"schema":"t","version":1,"error":{"code":"c","message":"m"},"extra":9}"#,
        );
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
            r#"{"schema":"t"}"#,             // no version, no body
            r#"{"version":1,"data":{}}"#,    // no schema
            r#"{"schema":"t","version":1}"#, // neither data nor error
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
        let l = Envelope::ok("t", 1, json!({"k": "v"}))
            .to_line()
            .expect("serialises");
        assert!(parse_line(&l).is_ok(), "must parse its own output verbatim");
    }
}
