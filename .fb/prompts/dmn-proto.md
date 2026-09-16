<!-- fb:creates crates/farmerbob-core/src/proto.rs -->
# Task: implement the daemon wire protocol

Rust workspace, already builds. Work only inside `crates/farmerbob-core`.

Create `crates/farmerbob-core/src/proto.rs`, declare from `lib.rs` with `pub mod proto;`.

## Context

`farmerbobd` owns all scheduling state; the `fb` CLI is a thin client talking to it
over a unix socket. The transport is newline-delimited JSON. The CLI's primary user is
another AI agent, so the schema is an API: breaking it breaks the planning agent's
scripts.

## What to implement

Two channel modes over one connection:

1. **Request/response**
   ```rust
   pub struct Request  { pub id: u64, pub method: Method, pub params: serde_json::Value }
   pub enum Response   { Ok { id: u64, result: serde_json::Value },
                         Err { id: u64, error: ProtoError } }
   pub struct ProtoError { pub code: ErrorCode, pub message: String }
   ```
   `ErrorCode` must include at least: `BadRequest`, `NotFound`, `WouldBlock`,
   `QuotaLimited`, `Conflict`, `Internal`, `VersionMismatch`.

2. **Subscription** — client sends `Subscribe { topics: Vec<Topic> }`, daemon pushes
   `Event`s. `Topic`: `Runs`, `Leases`, `Experiments`, `All`. `Event` variants:
   `RunStateChanged`, `LeaseGranted`, `LeaseReleased`, `ExperimentFinished`,
   `QuotaParked`, `Heartbeat`.

Also:

- `Method` enum covering: `Status`, `RunStart`, `RunKill`, `RunList`, `LeaseAcquire`,
  `LeaseRelease`, `LeaseStatus`, `ExpSubmit`, `ExpStatus`, `Grade`, `Leaderboard`.
- `pub const PROTO_VERSION: u32` and a `Hello { proto_version }` handshake, plus
  `fn check_version(theirs: u32) -> Result<(), ProtoError>` returning `VersionMismatch`
  with a message naming both versions.
- `fn encode<T: Serialize>(msg: &T) -> Result<String>` producing **exactly one line**
  (no interior newlines — serialize compact) terminated by `\n`.
- `fn decode_line<T: DeserializeOwned>(line: &str) -> Result<T>`.
- A `FrameReader` that accepts arbitrary byte chunks and yields complete lines,
  handling a message split across chunk boundaries and several messages in one chunk.

## Rules

- Tag enums explicitly for a stable wire format: `#[serde(tag = "type")]` on
  message enums, `#[serde(rename_all = "snake_case")]` throughout. An added variant
  must not silently change existing wire names.
- Unknown fields on incoming messages must NOT be a hard error (forward compatibility).
- Available deps: serde, serde_json, thiserror, chrono, uuid. Add none.
- Tests required for: round-tripping every `Event` variant; `encode` never emits an
  interior newline even when a string field contains `\n`; `FrameReader` reassembling
  a message split mid-token and splitting two messages in one chunk; `check_version`
  rejecting a mismatch.
- `cargo build -p farmerbob-core` and `cargo test -p farmerbob-core` must pass. Run
  them yourself and fix failures before finishing.

When done, briefly state what you implemented.

## How this will be scored

farmerbob re-runs everything itself; your self-report is not used. Stated so you can
optimise for the real bar rather than guess at it.

**Gate (all required, else the run scores as failed):**
- `cargo build -p <crate>` succeeds
- `cargo test -p <crate>` passes, with at least one test that actually executes
- only the crate named in the task is modified

**Scored, in this order:**
1. **Conformance** — a test suite you will not see, derived from this spec, is run against
   your implementation. The assertions are hidden; the criteria are exactly what this
   document states.
2. **Panic-freedom** — no `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` on
   any path reachable from input, outside `#[cfg(test)]`.
3. **`cargo clippy -- -D warnings` clean.**
4. **Test depth** — number of distinct behaviours covered, not number of assertions.
5. **Documentation** — `///` on every public item.
6. **Structure** — coherent modules over one large file, where the crate warrants it.

**Not scored:** wallclock. Taking longer to produce better work is the preferred trade.
There is a generous resource budget; a run is cut early only if it stops making progress or
regresses past its own best error count.
