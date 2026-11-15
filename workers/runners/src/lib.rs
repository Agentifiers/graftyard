//! `graft-runners`, the hosted-runner service in the runner account (`design/SHARED.md`, "Actions", Hosted runners;
//! `design/DESIGN.md`, Shape, `graft-runners`; `build/BUILD-PLAN.md`, M4-WP4). `control` decides what one pass does;
//! `worker` runs passes on the Workers runtime, on each `workflow_job` webhook and on the 5-minute Cron Trigger.

pub mod control;
#[cfg(target_arch = "wasm32")]
mod worker;
