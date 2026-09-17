//! Wire protocol for farmerbob daemon communication.
//!
//! The protocol uses newline-delimited JSON over a Unix socket. It supports
//! two channel modes over a single connection: request/response and
//! server-pushed event subscriptions.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Current protocol version. Increment on any breaking wire-format change.
pub const PROTO_VERSION: u32 = 1;

/// Sent by the client immediately after connecting.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub struct Hello {
    pub proto_version: u32,
}

/// Check that the peer's protocol version is compatible.
pub fn check_version(theirs: u32) -> Result<(), ProtoError> {
    if theirs == PROTO_VERSION {
        Ok(())
    } else {
        Err(ProtoError {
            code: ErrorCode::VersionMismatch,
            message: format!(
                "protocol version mismatch: expected {}, got {}",
                PROTO_VERSION, theirs
            ),
        })
    }
}

/// Error codes for protocol-level failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
#[error("{0}")]
pub enum ErrorCode {
    #[error("bad request")]
    BadRequest,
    #[error("not found")]
    NotFound,
    #[error("would block")]
    WouldBlock,
    #[error("quota limited")]
    QuotaLimited,
    #[error("conflict")]
    Conflict,
    #[error("internal error")]
    Internal,
    #[error("version mismatch")]
    VersionMismatch,
}

/// A protocol error returned in an error response.
#[derive(Debug, Clone, Serialize, Deserialize, Error)]
#[serde(rename_all = "snake_case")]
#[error("{code}: {message}")]
pub struct ProtoError {
    pub code: ErrorCode,
    pub message: String,
}

/// Methods the client can invoke on the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Method {
    Status,
    RunStart,
    RunKill,
    RunList,
    LeaseAcquire,
    LeaseRelease,
    LeaseStatus,
    ExpSubmit,
    ExpStatus,
    Grade,
    Leaderboard,
}

/// A request from client to daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Request {
    pub id: u64,
    pub method: Method,
    pub params: Value,
}

/// Successful response from daemon to client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ResponseOk {
    pub id: u64,
    pub result: Value,
}

/// Error response from daemon to client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ResponseErr {
    pub id: u64,
    pub error: ProtoError,
}

/// A response from the daemon (either success or error).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum Response {
    #[serde(rename = "ok")]
    Ok(ResponseOk),
    #[serde(rename = "err")]
    Err(ResponseErr),
}

/// Topics the client can subscribe to for server-pushed events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    Runs,
    Leases,
    Experiments,
    All,
}

/// Subscription request sent by client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub struct Subscribe {
    pub topics: Vec<Topic>,
}

/// Events pushed by the daemon to subscribed clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Event {
    RunStateChanged { run_id: String, state: String },
    LeaseGranted { lease_id: String, run_id: String },
    LeaseReleased { lease_id: String },
    ExperimentFinished { experiment_id: String },
    QuotaParked { run_id: String },
    Heartbeat { timestamp: String },
}

/// Serialize a message to a single compact JSON line terminated by `\n`.
pub fn encode<T: Serialize>(msg: &T) -> Result<String, serde_json::Error> {
    let mut json = serde_json::to_string(msg)?;
    json.push('\n');
    Ok(json)
}

/// Deserialize a single JSON line into a message.
pub fn decode_line<T: for<'de> Deserialize<'de>>(line: &str) -> Result<T, serde_json::Error> {
    serde_json::from_str(line)
}

/// A reader that reassembles complete lines from arbitrary byte chunks.
#[derive(Debug, Default)]
pub struct FrameReader {
    buffer: String,
}

impl FrameReader {
    /// Create a new empty frame reader.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a byte chunk into the reader, returning any complete lines.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<Vec<String>, std::str::Utf8Error> {
        let s = std::str::from_utf8(chunk)?;
        self.buffer.push_str(s);

        let mut lines = Vec::new();
        while let Some(pos) = self.buffer.find('\n') {
            let line = self.buffer[..pos].to_string();
            lines.push(line);
            self.buffer.drain(..=pos);
        }
        Ok(lines)
    }

    /// Returns any remaining incomplete line in the buffer.
    pub fn remaining(&self) -> &str {
        &self.buffer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_handshake() {
        let hello = Hello {
            proto_version: PROTO_VERSION,
        };
        let encoded = encode(&hello).unwrap();
        assert!(encoded.ends_with('\n'));
        assert!(!encoded[..encoded.len() - 1].contains('\n'));

        let decoded: Hello = decode_line(encoded.trim_end()).unwrap();
        assert_eq!(decoded.proto_version, PROTO_VERSION);
    }

    #[test]
    fn check_version_match() {
        assert!(check_version(PROTO_VERSION).is_ok());
    }

    #[test]
    fn check_version_mismatch() {
        let err = check_version(PROTO_VERSION + 1).unwrap_err();
        assert_eq!(err.code, ErrorCode::VersionMismatch);
        assert!(err.message.contains(&PROTO_VERSION.to_string()));
        assert!(err.message.contains(&(PROTO_VERSION + 1).to_string()));
    }

    #[test]
    fn encode_no_interior_newline() {
        let event = Event::Heartbeat {
            timestamp: "2024-01-01T00:00:00Z\ninjected".to_string(),
        };
        let encoded = encode(&event).unwrap();
        let lines: Vec<&str> = encoded.split('\n').collect();
        assert_eq!(lines.len(), 2); // message + empty after final \n
        assert!(!lines[0].contains('\n'));
    }

    #[test]
    fn request_response_roundtrip() {
        let req = Request {
            id: 42,
            method: Method::RunStart,
            params: serde_json::json!({ "task_id": "abc" }),
        };
        let encoded = encode(&req).unwrap();
        let decoded: Request = decode_line(encoded.trim_end()).unwrap();
        assert_eq!(decoded.id, req.id);
        assert!(matches!(decoded.method, Method::RunStart));
        assert_eq!(decoded.params, req.params);
    }

    #[test]
    fn response_ok_roundtrip() {
        let resp = Response::Ok(ResponseOk {
            id: 1,
            result: serde_json::json!({ "run_id": "run-123" }),
        });
        let encoded = encode(&resp).unwrap();
        let decoded: Response = decode_line(encoded.trim_end()).unwrap();
        assert!(matches!(decoded, Response::Ok(ref ok) if ok.id == 1));
    }

    #[test]
    fn response_err_roundtrip() {
        let resp = Response::Err(ResponseErr {
            id: 2,
            error: ProtoError {
                code: ErrorCode::NotFound,
                message: "run not found".to_string(),
            },
        });
        let encoded = encode(&resp).unwrap();
        let decoded: Response = decode_line(encoded.trim_end()).unwrap();
        assert!(
            matches!(decoded, Response::Err(ref err) if err.id == 2 && err.error.code == ErrorCode::NotFound)
        );
    }

    #[test]
    fn subscribe_roundtrip() {
        let sub = Subscribe {
            topics: vec![Topic::Runs, Topic::Leases],
        };
        let encoded = encode(&sub).unwrap();
        let decoded: Subscribe = decode_line(encoded.trim_end()).unwrap();
        assert_eq!(decoded.topics, sub.topics);
    }

    #[test]
    fn event_run_state_changed_roundtrip() {
        let evt = Event::RunStateChanged {
            run_id: "run-1".to_string(),
            state: "Running".to_string(),
        };
        let encoded = encode(&evt).unwrap();
        let decoded: Event = decode_line(encoded.trim_end()).unwrap();
        assert!(
            matches!(decoded, Event::RunStateChanged { run_id, state } if run_id == "run-1" && state == "Running")
        );
    }

    #[test]
    fn event_lease_granted_roundtrip() {
        let evt = Event::LeaseGranted {
            lease_id: "lease-1".to_string(),
            run_id: "run-1".to_string(),
        };
        let encoded = encode(&evt).unwrap();
        let decoded: Event = decode_line(encoded.trim_end()).unwrap();
        assert!(
            matches!(decoded, Event::LeaseGranted { lease_id, run_id } if lease_id == "lease-1" && run_id == "run-1")
        );
    }

    #[test]
    fn event_lease_released_roundtrip() {
        let evt = Event::LeaseReleased {
            lease_id: "lease-1".to_string(),
        };
        let encoded = encode(&evt).unwrap();
        let decoded: Event = decode_line(encoded.trim_end()).unwrap();
        assert!(matches!(decoded, Event::LeaseReleased { lease_id } if lease_id == "lease-1"));
    }

    #[test]
    fn event_experiment_finished_roundtrip() {
        let evt = Event::ExperimentFinished {
            experiment_id: "exp-1".to_string(),
        };
        let encoded = encode(&evt).unwrap();
        let decoded: Event = decode_line(encoded.trim_end()).unwrap();
        assert!(
            matches!(decoded, Event::ExperimentFinished { experiment_id } if experiment_id == "exp-1")
        );
    }

    #[test]
    fn event_quota_parked_roundtrip() {
        let evt = Event::QuotaParked {
            run_id: "run-1".to_string(),
        };
        let encoded = encode(&evt).unwrap();
        let decoded: Event = decode_line(encoded.trim_end()).unwrap();
        assert!(matches!(decoded, Event::QuotaParked { run_id } if run_id == "run-1"));
    }

    #[test]
    fn event_heartbeat_roundtrip() {
        let evt = Event::Heartbeat {
            timestamp: "2024-01-01T00:00:00Z".to_string(),
        };
        let encoded = encode(&evt).unwrap();
        let decoded: Event = decode_line(encoded.trim_end()).unwrap();
        assert!(
            matches!(decoded, Event::Heartbeat { timestamp } if timestamp == "2024-01-01T00:00:00Z")
        );
    }

    #[test]
    fn frame_reader_split_message() {
        let mut reader = FrameReader::new();
        let chunk1 = br#"{"type":"hello","proto_version":1}"#;
        let chunk2 = b"\n";
        let lines1 = reader.feed(chunk1).unwrap();
        assert!(lines1.is_empty());
        let lines2 = reader.feed(chunk2).unwrap();
        assert_eq!(lines2, vec![r#"{"type":"hello","proto_version":1}"#]);
    }

    #[test]
    fn frame_reader_two_messages_one_chunk() {
        let mut reader = FrameReader::new();
        let chunk = br#"{"type":"hello","proto_version":1}
{"type":"subscribe","topics":["runs"]}
"#;
        let lines = reader.feed(chunk).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], r#"{"type":"hello","proto_version":1}"#);
        assert_eq!(lines[1], r#"{"type":"subscribe","topics":["runs"]}"#);
    }

    #[test]
    fn frame_reader_partial_then_complete() {
        let mut reader = FrameReader::new();
        let chunk1 = br#"{"id":1,"method":{"type":"status"},"params":{}"#;
        let chunk2 = b"}\n";
        let lines1 = reader.feed(chunk1).unwrap();
        assert!(lines1.is_empty());
        let lines2 = reader.feed(chunk2).unwrap();
        assert_eq!(lines2.len(), 1);
        let req: Request = decode_line(&lines2[0]).unwrap();
        assert_eq!(req.id, 1);
    }

    #[test]
    fn error_code_serialization() {
        let code = ErrorCode::BadRequest;
        let encoded = serde_json::to_string(&code).unwrap();
        assert_eq!(encoded, "\"bad_request\"");
    }

    #[test]
    fn proto_error_serialization() {
        let err = ProtoError {
            code: ErrorCode::Internal,
            message: "something broke".to_string(),
        };
        let encoded = serde_json::to_string(&err).unwrap();
        let decoded: ProtoError = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.code, ErrorCode::Internal);
        assert_eq!(decoded.message, "something broke");
    }

    #[test]
    fn unknown_fields_ignored() {
        // Forward compatibility: unknown fields should not cause deserialization failure
        let json = r#"{"id":1,"method":{"type":"status"},"params":{},"unknown_field":123}"#;
        let req: Request = decode_line(json).unwrap();
        assert_eq!(req.id, 1);
    }

    #[test]
    fn method_enum_variants() {
        let methods = vec![
            Method::Status,
            Method::RunStart,
            Method::RunKill,
            Method::RunList,
            Method::LeaseAcquire,
            Method::LeaseRelease,
            Method::LeaseStatus,
            Method::ExpSubmit,
            Method::ExpStatus,
            Method::Grade,
            Method::Leaderboard,
        ];
        for method in methods {
            let encoded = encode(&method).unwrap();
            let decoded: Method = decode_line(encoded.trim_end()).unwrap();
            assert_eq!(format!("{:?}", method), format!("{:?}", decoded));
        }
    }

    #[test]
    fn topic_all_serialization() {
        let topic = Topic::All;
        let encoded = serde_json::to_string(&topic).unwrap();
        assert_eq!(encoded, "\"all\"");
    }
}
