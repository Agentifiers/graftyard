//! The Actions engine (`build/BUILD-PLAN.md`, M4-WP3; `design/SHARED.md`, "Actions runs and jobs" and "Actions";
//! `design/DESIGN.md`, Shape, "Identity"). Pure Rust: no Workers dependency, no clock and no randomness but what the
//! caller passes in. The design as built is `tests/m4/results/m4-wp3-design.md`.

pub mod broker;
pub mod hosted;
pub mod ids;
pub mod job;
pub mod labels;
pub mod relay;
pub mod rest;
pub mod run_owner;
pub mod runsvc;
pub mod shard;
pub mod specs;
pub mod trigger;
pub mod workflow;

pub use ids::{ACTIONS_SHARDS, JobId, OWNER_SCOPE_SHARD, RunId, RunnerId, RunnerSessionId, actions_shard};
pub use job::{
    Conclusion, IllegalTransition, JOB_LEASE, JobEvent, JobState, LEASE_REWRITE_UNDER, QUEUE_LIMIT, RunStatus,
    derive_run_status, transition,
};
pub use labels::{LabelError, LabelSet};
