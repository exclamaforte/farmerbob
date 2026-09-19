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
pub mod adjudicate;
pub mod admit_queue;
pub mod agent;
pub mod attempt_log;
pub mod attention;
pub mod autopilot;
pub mod availability;
pub mod backfill;
pub mod graft;
pub mod board;
pub mod budget;
pub mod build_verdict;
pub mod cell_record;
pub mod compare;
pub mod confinement;
pub mod conform;
pub mod corpus;
pub mod cost;
pub mod critique_plan;
pub mod crossx;
pub mod divergence;
pub mod envelope;
pub mod error;
pub mod escalate_gate;
pub mod experiment;
pub mod field_shape;
pub mod finding_fate;
pub mod gate;
pub mod grade;
pub mod grading;
pub mod ids;
pub mod known_defect;
pub mod lease;
pub mod lease_manager;
pub mod ledger;
pub mod lib_diff;
pub mod limit_signal;
pub mod liveness;
pub mod measurement;
pub mod mutate;
pub mod orphan_check;
pub mod outcome;
pub mod park_decision;
pub mod precondition;
pub mod price_update;
pub mod pricing;
pub mod prior;
pub mod promote;
pub mod prove_veto;
pub mod proto;
pub mod quota;
pub mod reap_exec;
pub mod reap_plan;
pub mod resource;
pub mod resume;
pub mod resume_guard;
pub mod reviewer;
pub mod roster;
pub mod router;
pub mod rubric_carry;
pub mod run;
pub mod run_state_view;
pub mod run_timing;
pub mod runstate;
pub mod scope;
pub mod selection;
pub mod semaphore;
pub mod sensitivity;
pub mod slots;
pub mod speclint_shape;
pub mod stage_exit;
pub mod stage_key;
pub mod stage_outcome;
pub mod suite_match;
pub mod suite_verdict;
pub mod task;
pub mod target_decl;
pub mod task_contract;
pub mod taskid;
pub mod testout;
pub mod timing;
pub mod verify_plan;
pub mod verifier;
pub mod vrouter;
pub mod wave_compose;
pub mod wave_plan;
pub mod window;
pub mod witness;
pub mod wtalloc;
pub mod wtreap;

pub use agent::{Agent, AgentKind};
pub use error::DomainError;
pub use experiment::Experiment;
pub use grade::{Grade, RubricScores};
pub use ids::{AgentId, ExperimentId, LeaseId, RunId, TaskId};
pub use lease::{Lease, LeaseClass};
pub use proto::{
    ErrorCode, Event, FrameReader, Hello, Method, PROTO_VERSION, ProtoError, Request, Response,
    ResponseErr, ResponseOk, Subscribe, Topic, check_version, decode_line, encode,
};
pub use resource::{ResourceSpec, Slot};
pub use run::{Run, RunState};
pub use task::Task;
