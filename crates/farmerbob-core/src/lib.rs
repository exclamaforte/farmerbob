//! farmerbob-core: pure domain types for the farmerbob agent orchestrator.
//!
//! farmerbob runs many AI coding agents in parallel on one machine, confines
//! each to a resource budget, serializes their access to a single GPU through
//! leases, and scores the resulting work. The types in this crate describe
//! that world.
//!
//! This crate performs no I/O: no filesystem, network, or process access.
//! It is plain data plus pure functions over that data.

#![forbid(unsafe_code)]

pub mod adapter;
pub mod agent;
pub mod error;
pub mod experiment;
pub mod grade;
pub mod ids;
pub mod lease;
pub mod proto;
pub mod resource;
pub mod run;
pub mod task;
pub mod taskid;
pub mod vrouter;

pub use agent::{Agent, AgentKind};
pub use error::DomainError;
pub use experiment::Experiment;
pub use grade::{Grade, RubricScores};
pub use ids::{AgentId, ExperimentId, LeaseId, RunId, TaskId};
pub use lease::{Lease, LeaseClass};
pub use proto::{
    check_version, decode_line, encode, ErrorCode, Event, FrameReader, Hello, Method, ProtoError,
    PROTO_VERSION, Request, Response, ResponseErr, ResponseOk, Subscribe, Topic,
};
pub use resource::{ResourceSpec, Slot};
pub use run::{Run, RunState};
pub use task::Task;
