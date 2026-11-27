//! SHARED's job state machine ("Actions runs and jobs"): `JobState`, `JobEvent`, the pure `transition`, and the run
//! status `RunOwner` derives from its jobs.

use std::time::Duration;

use graftyard_types::Timestamp;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::ids::{JobId, RunnerId};
use crate::labels::LabelSet;

/// A lease ends this long after its last write with no `renewjob` (SHARED, Leases).
pub const JOB_LEASE: Duration = Duration::from_secs(600);
/// `renewjob` rewrites the lease only when less than this remains, so a runner renewing every 60 s [measured,
/// `tests/m4/wp3/runner-protocol.md`] writes about once every 5 minutes (SHARED, Few writes per job).
pub const LEASE_REWRITE_UNDER: Duration = Duration::from_secs(300);
/// A job `Queued` this long gets `QueueExpired` (SHARED, Queue limit) [unverified: GitHub's limit].
pub const QUEUE_LIMIT: Duration = Duration::from_secs(24 * 3_600);

fn after(t: Timestamp, d: Duration) -> Timestamp {
    Timestamp(t.0 + d.as_millis() as u64)
}

/// Wire tags: postcard stores and sends the variants by position (`crates/git/tests/wire_tags.golden`). Add a variant
/// only at the end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Conclusion {
    Success,
    Failure,
    Cancelled,
    Skipped,
    TimedOut,
    StartupFailure,
}

/// Wire tags as `Conclusion`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    Pending {
        needs: SmallVec<[JobId; 4]>,
    },
    Queued {
        labels: LabelSet,
        since: Timestamp,
    },
    Assigned {
        runner: RunnerId,
        lease_until: Timestamp,
    },
    InProgress {
        runner: RunnerId,
        lease_until: Timestamp,
    },
    Completed {
        conclusion: Conclusion,
        at: Timestamp,
    },
    /// `agent-ci` (0x15.13): a job of an agent run held for a writer's approval (`ActionsEvent::RunHeld`). `needs` is
    /// what `Pending` would hold, and `since` when the run was held; `Approved` releases it to `Queued`, or to
    /// `Pending` while it has needs.
    Held {
        needs: SmallVec<[JobId; 4]>,
        since: Timestamp,
    },
    /// `secrets-wait` (0x15.14, F142): a job of an agent run whose steps reference a secret
    /// (`crate::secrets::references`), waiting for `AgentGate` on `head`, the agent's commit the run is for. No runner
    /// takes it; `GateHeld` releases it to `Queued`, or to `Pending` while it has needs, and only then may it read
    /// secrets (`crate::secrets::reads_secrets`).
    SecretsWait {
        needs: SmallVec<[JobId; 4]>,
        since: Timestamp,
        head: [u8; 20],
    },
}

impl JobState {
    pub fn completed(&self) -> Option<Conclusion> {
        match self {
            JobState::Completed { conclusion, .. } => Some(*conclusion),
            _ => None,
        }
    }

    /// The runner holding the job, if one does.
    pub fn runner(&self) -> Option<RunnerId> {
        match self {
            JobState::Assigned { runner, .. } | JobState::InProgress { runner, .. } => Some(*runner),
            _ => None,
        }
    }

    /// When the leader's alarm must act on this job: a lease end, or the queue limit.
    pub fn deadline(&self) -> Option<Timestamp> {
        match self {
            JobState::Queued { since, .. } | JobState::SecretsWait { since, .. } => Some(after(*since, QUEUE_LIMIT)),
            JobState::Assigned { lease_until, .. } | JobState::InProgress { lease_until, .. } => Some(*lease_until),
            _ => None,
        }
    }
}

/// Wire tags as `Conclusion`. `Renewed` carries no time: the new lease is the entry's time plus `JOB_LEASE`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobEvent {
    NeedsMet,
    NeedsFailed,
    Matched(RunnerId),
    Acquired,
    Renewed,
    Finished(Conclusion),
    LeaseExpired,
    QueueExpired,
    Cancel,
    /// `agent-ci` (0x15.13): a writer approved the job's held run (`ActionsOp::ApproveRun`).
    Approved,
    /// `secrets-wait` (0x15.14): `AgentGate` holds for the waiting job's head (`ActionsOp::ReleaseSecrets`).
    GateHeld,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[error("{event} is not a transition from {from}")]
pub struct IllegalTransition {
    pub from: &'static str,
    pub event: &'static str,
}

fn state_name(s: &JobState) -> &'static str {
    match s {
        JobState::Pending { .. } => "Pending",
        JobState::Queued { .. } => "Queued",
        JobState::Assigned { .. } => "Assigned",
        JobState::InProgress { .. } => "InProgress",
        JobState::Completed { .. } => "Completed",
        JobState::Held { .. } => "Held",
        JobState::SecretsWait { .. } => "SecretsWait",
    }
}

fn event_name(e: &JobEvent) -> &'static str {
    match e {
        JobEvent::NeedsMet => "NeedsMet",
        JobEvent::NeedsFailed => "NeedsFailed",
        JobEvent::Matched(_) => "Matched",
        JobEvent::Acquired => "Acquired",
        JobEvent::Renewed => "Renewed",
        JobEvent::Finished(_) => "Finished",
        JobEvent::LeaseExpired => "LeaseExpired",
        JobEvent::QueueExpired => "QueueExpired",
        JobEvent::Cancel => "Cancel",
        JobEvent::Approved => "Approved",
        JobEvent::GateHeld => "GateHeld",
    }
}

/// SHARED's pure transition table. `labels` is the job's spec, which `NeedsMet` and a lease lost before `acquirejob`
/// queue it with; `now` is the entry's time.
///
/// - `LeaseExpired` before the runner took the job (`Assigned`) puts it back in the queue; after (`InProgress`) it
///   fails it, as GitHub's infrastructure failure does. It is refused before the lease ends, so a late or early
///   alarm cannot fail a live job.
/// - `Renewed` returns the state unchanged while more than `LEASE_REWRITE_UNDER` remains: the caller writes nothing.
///   So does `Acquired` on a job already `InProgress`, which only its own runner may send.
/// - `Cancel` completes any unfinished job `Cancelled`; a completed job refuses every event.
pub fn transition(
    s: &JobState,
    e: &JobEvent,
    labels: &LabelSet,
    now: Timestamp,
) -> Result<JobState, IllegalTransition> {
    use JobEvent as E;
    use JobState as S;
    let lease = after(now, JOB_LEASE);
    let done = |conclusion| S::Completed { conclusion, at: now };
    let next = match (s, e) {
        (S::Completed { .. }, _) => None,
        (_, E::Cancel) => Some(done(Conclusion::Cancelled)),
        (S::Pending { .. }, E::NeedsMet) => Some(S::Queued { labels: labels.clone(), since: now }),
        (S::Held { needs, .. }, E::Approved) if needs.is_empty() => {
            Some(S::Queued { labels: labels.clone(), since: now })
        }
        (S::Held { needs, .. }, E::Approved) => Some(S::Pending { needs: needs.clone() }),
        (S::SecretsWait { needs, .. }, E::GateHeld) if needs.is_empty() => {
            Some(S::Queued { labels: labels.clone(), since: now })
        }
        (S::SecretsWait { needs, .. }, E::GateHeld) => Some(S::Pending { needs: needs.clone() }),
        (S::Pending { .. } | S::SecretsWait { .. }, E::NeedsFailed) => Some(done(Conclusion::Skipped)),
        (S::Queued { .. }, E::Matched(r)) => Some(S::Assigned { runner: *r, lease_until: lease }),
        (S::Queued { since, .. } | S::SecretsWait { since, .. }, E::QueueExpired)
            if now >= after(*since, QUEUE_LIMIT) =>
        {
            Some(done(Conclusion::Failure))
        }
        (S::Assigned { runner, .. }, E::Acquired) => Some(S::InProgress { runner: *runner, lease_until: lease }),
        // A repeated acquire by the runner that holds the job (the shard checks who): it writes nothing, and the run
        // service answers the job message again. The runner retries an acquire whose answer it lost and drops the job
        // on a 409 (`decisions/OPEN-QUESTIONS.md`, F77).
        (S::InProgress { .. }, E::Acquired) => Some(s.clone()),
        (S::Assigned { lease_until, .. }, E::LeaseExpired) if now >= *lease_until => {
            Some(S::Queued { labels: labels.clone(), since: now })
        }
        (S::InProgress { runner, lease_until }, E::Renewed) => {
            if lease_until.0 < now.0 + LEASE_REWRITE_UNDER.as_millis() as u64 {
                Some(S::InProgress { runner: *runner, lease_until: lease })
            } else {
                Some(s.clone())
            }
        }
        (S::InProgress { lease_until, .. }, E::LeaseExpired) if now >= *lease_until => Some(done(Conclusion::Failure)),
        (S::InProgress { .. } | S::Assigned { .. }, E::Finished(c)) => Some(done(*c)),
        _ => None,
    };
    next.ok_or(IllegalTransition { from: state_name(s), event: event_name(e) })
}

/// Wire tags as `Conclusion`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunStatus {
    Queued,
    InProgress,
    Completed(Conclusion),
    /// `agent-ci` (0x15.13): every unfinished job is held for a writer's approval. GHES renders it `completed` with
    /// conclusion `action_required` (`crate::rest`).
    Held,
}

/// The run's status from its jobs (SHARED). Completed when every job is, with GitHub's precedence: any failure fails
/// the run, then a cancel cancels it, then a timeout, and a run whose jobs all skipped or succeeded succeeds unless
/// every job skipped [inference: GitHub's run conclusion, which SHARED does not spell out]. In progress once any job
/// left the queue; queued otherwise.
pub fn derive_run_status(jobs: &[JobState]) -> RunStatus {
    if jobs.is_empty() {
        return RunStatus::Completed(Conclusion::Skipped);
    }
    let done: Vec<Conclusion> = jobs.iter().filter_map(JobState::completed).collect();
    if done.len() == jobs.len() {
        let has = |c| done.contains(&c);
        let c = if has(Conclusion::Failure) || has(Conclusion::StartupFailure) {
            Conclusion::Failure
        } else if has(Conclusion::Cancelled) {
            Conclusion::Cancelled
        } else if has(Conclusion::TimedOut) {
            Conclusion::TimedOut
        } else if done.iter().all(|c| *c == Conclusion::Skipped) {
            Conclusion::Skipped
        } else {
            Conclusion::Success
        };
        return RunStatus::Completed(c);
    }
    if jobs.iter().any(|j| matches!(j, JobState::Held { .. })) {
        return RunStatus::Held;
    }
    let started = jobs
        .iter()
        .any(|j| matches!(j, JobState::Assigned { .. } | JobState::InProgress { .. } | JobState::Completed { .. }));
    if started { RunStatus::InProgress } else { RunStatus::Queued }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::RunnerId;
    use graftyard_types::DbId;

    fn t(s: u64) -> Timestamp {
        Timestamp(s * 1_000)
    }
    fn labels() -> LabelSet {
        LabelSet::parse(["self-hosted", "linux"]).unwrap()
    }
    const R: RunnerId = RunnerId(DbId(7));

    #[test]
    fn a_job_runs_from_pending_to_completed_and_a_completed_job_takes_nothing() {
        let l = labels();
        let p = JobState::Pending { needs: SmallVec::new() };
        let q = transition(&p, &JobEvent::NeedsMet, &l, t(1)).unwrap();
        assert_eq!(q, JobState::Queued { labels: l.clone(), since: t(1) });
        let a = transition(&q, &JobEvent::Matched(R), &l, t(2)).unwrap();
        assert_eq!(a, JobState::Assigned { runner: R, lease_until: t(602) });
        let i = transition(&a, &JobEvent::Acquired, &l, t(3)).unwrap();
        let c = transition(&i, &JobEvent::Finished(Conclusion::Success), &l, t(9)).unwrap();
        assert_eq!(c, JobState::Completed { conclusion: Conclusion::Success, at: t(9) });
        for e in
            [JobEvent::Cancel, JobEvent::Matched(R), JobEvent::Finished(Conclusion::Failure), JobEvent::LeaseExpired]
        {
            assert!(transition(&c, &e, &l, t(10)).is_err(), "{e:?} after Completed");
        }
        // A queued job is matched once: a second runner's match is refused.
        assert!(transition(&a, &JobEvent::Matched(RunnerId(DbId(8))), &l, t(2)).is_err());
    }

    #[test]
    fn a_held_job_waits_for_approval_then_queues_or_waits_for_its_needs() {
        let l = labels();
        let need: SmallVec<[JobId; 4]> = smallvec::smallvec![JobId(DbId(3))];
        let free = JobState::Held { needs: SmallVec::new(), since: t(1) };
        let blocked = JobState::Held { needs: need.clone(), since: t(1) };
        // No runner takes a held job, and its needs being met does not release it.
        for e in [JobEvent::Matched(R), JobEvent::NeedsMet, JobEvent::QueueExpired, JobEvent::Acquired] {
            assert!(transition(&free, &e, &l, t(9_000)).is_err(), "{e:?} on a held job");
        }
        assert_eq!(
            transition(&free, &JobEvent::Approved, &l, t(5)).unwrap(),
            JobState::Queued { labels: l.clone(), since: t(5) }
        );
        assert_eq!(transition(&blocked, &JobEvent::Approved, &l, t(5)).unwrap(), JobState::Pending { needs: need });
        assert_eq!(
            transition(&blocked, &JobEvent::Cancel, &l, t(6)).unwrap(),
            JobState::Completed { conclusion: Conclusion::Cancelled, at: t(6) }
        );
        // Approval releases only a held job.
        let q = JobState::Queued { labels: l.clone(), since: t(1) };
        assert!(transition(&q, &JobEvent::Approved, &l, t(5)).is_err());
        // The run reads held while any job is, even beside a finished one; a run all of whose jobs finished is not.
        let done = JobState::Completed { conclusion: Conclusion::Success, at: t(2) };
        assert_eq!(derive_run_status(&[done.clone(), blocked.clone()]), RunStatus::Held);
        assert_eq!(derive_run_status(&[done.clone(), q]), RunStatus::InProgress);
        assert_eq!(derive_run_status(&[done]), RunStatus::Completed(Conclusion::Success));
    }

    /// `secrets-wait` (0x15.14): a job waiting for `AgentGate` takes no runner and no `NeedsMet`; `GateHeld` queues it
    /// or, with needs, makes it pending; a failed need skips it, it fails at the queue limit and not before, and a
    /// stall moves its queue time as a queued job's.
    #[test]
    fn a_secrets_waiting_job_takes_no_runner_until_the_gate_then_queues_or_waits_for_its_needs() {
        let l = labels();
        let need: SmallVec<[JobId; 4]> = smallvec::smallvec![JobId(DbId(3))];
        let free = JobState::SecretsWait { needs: SmallVec::new(), since: t(1), head: [7; 20] };
        let blocked = JobState::SecretsWait { needs: need.clone(), since: t(1), head: [7; 20] };
        for e in [JobEvent::Matched(R), JobEvent::NeedsMet, JobEvent::Acquired, JobEvent::Approved] {
            assert!(transition(&free, &e, &l, t(5)).is_err(), "{e:?} on a waiting job");
        }
        assert_eq!(
            transition(&free, &JobEvent::GateHeld, &l, t(5)).unwrap(),
            JobState::Queued { labels: l.clone(), since: t(5) }
        );
        assert_eq!(transition(&blocked, &JobEvent::GateHeld, &l, t(5)).unwrap(), JobState::Pending { needs: need });
        assert_eq!(
            transition(&blocked, &JobEvent::NeedsFailed, &l, t(6)).unwrap().completed(),
            Some(Conclusion::Skipped)
        );
        let limit = QUEUE_LIMIT.as_secs() + 1;
        assert!(transition(&free, &JobEvent::QueueExpired, &l, t(limit - 1)).is_err(), "one second early");
        assert_eq!(
            transition(&free, &JobEvent::QueueExpired, &l, t(limit)).unwrap().completed(),
            Some(Conclusion::Failure)
        );
        assert_eq!(free.deadline(), Some(t(limit)));
        // Only a waiting job is released by the gate.
        let q = JobState::Queued { labels: l.clone(), since: t(1) };
        assert!(transition(&q, &JobEvent::GateHeld, &l, t(5)).is_err());
        assert!(
            transition(&JobState::Held { needs: SmallVec::new(), since: t(1) }, &JobEvent::GateHeld, &l, t(5)).is_err()
        );
        assert_eq!(
            derive_run_status(std::slice::from_ref(&free)),
            RunStatus::Queued,
            "a run of waiting jobs reads queued"
        );
    }

    #[test]
    fn a_lease_ends_only_at_its_time_and_fails_only_a_started_job() {
        let l = labels();
        let i = JobState::InProgress { runner: R, lease_until: t(600) };
        assert!(transition(&i, &JobEvent::LeaseExpired, &l, t(599)).is_err(), "one second early");
        assert_eq!(
            transition(&i, &JobEvent::LeaseExpired, &l, t(600)).unwrap(),
            JobState::Completed { conclusion: Conclusion::Failure, at: t(600) }
        );
        let a = JobState::Assigned { runner: R, lease_until: t(600) };
        assert_eq!(
            transition(&a, &JobEvent::LeaseExpired, &l, t(601)).unwrap(),
            JobState::Queued { labels: l.clone(), since: t(601) }
        );
    }

    #[test]
    fn a_renew_rewrites_the_lease_only_when_under_five_minutes_remain() {
        let l = labels();
        let i = JobState::InProgress { runner: R, lease_until: t(600) };
        // 60 s in: 540 s remain, over 300, so nothing changes.
        assert_eq!(transition(&i, &JobEvent::Renewed, &l, t(60)).unwrap(), i);
        // 301 s in: 299 s remain, so the lease moves to now + 600.
        assert_eq!(
            transition(&i, &JobEvent::Renewed, &l, t(301)).unwrap(),
            JobState::InProgress { runner: R, lease_until: t(901) }
        );
        // A 2-hour job renewing every 60 s writes its lease about 24 times.
        let (mut s, mut writes) = (JobState::InProgress { runner: R, lease_until: t(600) }, 0);
        for k in 1..=120 {
            let n = transition(&s, &JobEvent::Renewed, &l, t(k * 60)).unwrap();
            if n != s {
                writes += 1;
            }
            s = n;
        }
        assert!((20..=26).contains(&writes), "{writes} lease writes in 2 h");
    }

    #[test]
    fn the_queue_limit_fails_a_job_after_24_hours_and_not_before() {
        let l = labels();
        let q = JobState::Queued { labels: l.clone(), since: t(0) };
        assert!(transition(&q, &JobEvent::QueueExpired, &l, t(86_399)).is_err());
        assert_eq!(
            transition(&q, &JobEvent::QueueExpired, &l, t(86_400)).unwrap().completed(),
            Some(Conclusion::Failure)
        );
        assert_eq!(q.deadline(), Some(t(86_400)));
    }

    #[test]
    fn a_cancel_completes_any_unfinished_job_cancelled() {
        let l = labels();
        for s in [
            JobState::Pending { needs: SmallVec::new() },
            JobState::Queued { labels: l.clone(), since: t(0) },
            JobState::Assigned { runner: R, lease_until: t(600) },
            JobState::InProgress { runner: R, lease_until: t(600) },
        ] {
            assert_eq!(transition(&s, &JobEvent::Cancel, &l, t(5)).unwrap().completed(), Some(Conclusion::Cancelled));
        }
    }

    #[test]
    fn run_status_takes_failure_over_cancel_over_success() {
        let l = labels();
        let done = |c| JobState::Completed { conclusion: c, at: t(1) };
        let q = JobState::Queued { labels: l, since: t(0) };
        assert_eq!(derive_run_status(&[q.clone(), q.clone()]), RunStatus::Queued);
        assert_eq!(derive_run_status(&[done(Conclusion::Success), q]), RunStatus::InProgress);
        assert_eq!(
            derive_run_status(&[done(Conclusion::Success), done(Conclusion::Cancelled), done(Conclusion::Failure)]),
            RunStatus::Completed(Conclusion::Failure)
        );
        assert_eq!(
            derive_run_status(&[done(Conclusion::Success), done(Conclusion::Cancelled)]),
            RunStatus::Completed(Conclusion::Cancelled)
        );
        assert_eq!(
            derive_run_status(&[done(Conclusion::Success), done(Conclusion::Skipped)]),
            RunStatus::Completed(Conclusion::Success)
        );
        assert_eq!(derive_run_status(&[done(Conclusion::Skipped)]), RunStatus::Completed(Conclusion::Skipped));
    }
}
