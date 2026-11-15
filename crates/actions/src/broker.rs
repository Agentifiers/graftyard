//! The `RunnerBroker`'s choice for one poll (`design/SHARED.md`, "Actions runs and jobs", Label matching), pure so the
//! site's broker and the in-memory gate run the same rule. A `Repo`-scope runner reads its repository's shard; an
//! `Owner`-scope runner reads all 16 of the owner's shards (BF-55).
//!
//! A runner that holds a job `Assigned` for more than `RESEND_AFTER` is handed that job's request again: the match
//! committed, but its answer was lost with a broker or a site, and without it the job would wait out its 10-minute
//! lease (BF-55's fail column is a dispatch stalled over 60 s). The runner acquires within a second of a delivered
//! match [measured], so a resend never races its first `acquirejob`; an ephemeral runner that already took a job skips
//! a second request, and a second `acquirejob` of a job it took is refused 409, which the runner skips [measured,
//! `tests/m4/wp3/runner-protocol.md`]. A runner holding any other unfinished job gets nothing (its listener polls while
//! its worker runs). Otherwise the oldest queued job whose labels the runner takes, over every shard read.

use serde::{Deserialize, Serialize};

use crate::ids::{JobId, RunnerId};
use std::time::Duration;

use crate::job::{JOB_LEASE, JobState};
use crate::labels::LabelSet;
use crate::shard::ActionsFold;
use graftyard_types::Timestamp;

/// One shard's part of a broker's table: its queued jobs and the runners that hold an unfinished job.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    /// `(job, labels, since, version)`, oldest first.
    pub queued: Vec<(JobId, LabelSet, Timestamp, u64)>,
    /// `(runner, job, assigned_at)`: `assigned_at` is when an `Assigned` job was matched, `None` once `InProgress`.
    pub held: Vec<(RunnerId, JobId, Option<Timestamp>)>,
}

/// How long a match may sit `Assigned` before its runner's next poll is handed it again [guess: the runner acquires
/// within a second of a delivered match, measured; 30 s leaves BF-55's 60 s].
pub const RESEND_AFTER: Duration = Duration::from_secs(30);

impl ActionsFold {
    pub fn summary(&self) -> Summary {
        let held = self
            .jobs
            .iter()
            .filter_map(|(id, j)| match &j.state {
                JobState::Assigned { runner, lease_until } => {
                    Some((*runner, *id, Some(Timestamp(lease_until.0.saturating_sub(JOB_LEASE.as_millis() as u64)))))
                }
                JobState::InProgress { runner, .. } => Some((*runner, *id, None)),
                _ => None,
            })
            .collect();
        let queued = self.queued().into_iter().map(|(j, l, s, v)| (j, l.clone(), s, v)).collect();
        Summary { queued, held }
    }
}

/// What a poll does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    /// The runner holds an unfinished job: no message.
    Busy,
    /// The runner's job on shard `shard` has sat `Assigned` past `RESEND_AFTER`: its request again.
    Resend { shard: u8, job: JobId },
    /// Commit `Matched(runner)` for this job at this version on shard `shard`.
    Match { shard: u8, job: JobId, version: u64 },
    /// Nothing queued that the runner takes.
    Idle,
}

/// The poll's choice at `now` over the shards read, each `(shard, summary)`.
pub fn pick(shards: &[(u8, &Summary)], runner: RunnerId, labels: &LabelSet, now: Timestamp) -> Pick {
    for (s, sum) in shards {
        if let Some((_, job, at)) = sum.held.iter().find(|(r, ..)| *r == runner) {
            let stuck = at.is_some_and(|a| now.0 >= a.0 + RESEND_AFTER.as_millis() as u64);
            return if stuck { Pick::Resend { shard: *s, job: *job } } else { Pick::Busy };
        }
    }
    shards
        .iter()
        .flat_map(|(s, sum)| sum.queued.iter().map(move |q| (*s, q)))
        .filter(|(_, (_, l, _, _))| l.fits(labels))
        .min_by_key(|(s, (job, _, since, _))| (*since, *s, *job))
        .map_or(Pick::Idle, |(shard, (job, _, _, version))| Pick::Match { shard, job: *job, version: *version })
}

#[cfg(test)]
mod tests {
    use super::*;
    use graftyard_types::DbId;

    fn l(xs: &[&str]) -> LabelSet {
        LabelSet::parse(xs).unwrap()
    }

    #[test]
    fn the_oldest_fitting_job_over_every_shard_and_a_held_job_first() {
        let r = RunnerId(DbId(9));
        let me = l(&["self-hosted", "linux"]);
        let a = Summary {
            queued: vec![
                (JobId(DbId(1)), l(&["gpu"]), Timestamp(10), 0),
                (JobId(DbId(2)), l(&["linux"]), Timestamp(30), 4),
            ],
            held: vec![(RunnerId(DbId(8)), JobId(DbId(5)), Some(Timestamp(0)))],
        };
        let b = Summary { queued: vec![(JobId(DbId(3)), l(&["self-hosted"]), Timestamp(20), 1)], held: vec![] };
        let now = Timestamp(100_000);
        // The oldest job, 1, does not fit; of those that do, shard 7's job 3 is older than shard 0's job 2.
        assert_eq!(pick(&[(0, &a), (7, &b)], r, &me, now), Pick::Match { shard: 7, job: JobId(DbId(3)), version: 1 });
        // Another runner's held job is not this one's.
        assert_eq!(pick(&[(0, &a)], r, &me, now), Pick::Match { shard: 0, job: JobId(DbId(2)), version: 4 });
        let held = |at| Summary { held: vec![(r, JobId(DbId(6)), at)], ..Summary::default() };
        // Matched 30 s ago: resent. Matched 29.999 s ago, or acquired: nothing, even with fitting jobs queued.
        let stuck = held(Some(Timestamp(now.0 - 30_000)));
        let fresh = held(Some(Timestamp(now.0 - 29_999)));
        let running = held(None);
        assert_eq!(pick(&[(0, &a), (7, &b), (3, &stuck)], r, &me, now), Pick::Resend { shard: 3, job: JobId(DbId(6)) });
        assert_eq!(pick(&[(0, &a), (7, &b), (3, &fresh)], r, &me, now), Pick::Busy);
        assert_eq!(pick(&[(0, &a), (7, &b), (3, &running)], r, &me, now), Pick::Busy);
        assert_eq!(pick(&[(0, &Summary::default())], r, &me, now), Pick::Idle);
    }
}
