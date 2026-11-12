//! The activity feed (`design/AGENT-LAYER.md`, Shape, "Activity feed", and "API reference"; `build/BUILD-PLAN.md`,
//! AM1-WP4): a fold over the group's log past a cursor, served by learners, or followers while none answers, never by
//! the leader.
//!
//! Pure. `events` turns one committed entry into feed events by comparing the core tier before and after it, so a
//! lease that expires with no command appears at the first entry whose `at` is at or after its `expires`, and one
//! released because its task went final appears in the task's entry. `Poll` and `Serving` are the held-poll rules: a
//! member holds polls only while it follows, answers held polls at once when it stops, refuses new ones then, and holds
//! at most FEED_HELD_MAX.

use std::time::Duration;

use graftyard_fold::Read;
use graftyard_identity::{AuthzRead, SessionId};
use graftyard_types::{Index, RepoId, Timestamp};
use smallvec::SmallVec;

use crate::issues::{IssueChange, IssueNumber};
use crate::lease::{Lease, LeaseId, LeaseRow, LeaseView};
use crate::refs::{ObjectId, RefName};
use crate::repo::RepoEvent;
use crate::task::TaskApiState;

pub const FEED_WAIT_MAX: Duration = Duration::from_secs(25);
pub const FEED_BATCH: Duration = Duration::from_secs(1);
/// [guess]
pub const FEED_BATCH_ALL: Duration = Duration::from_secs(10);
/// Concurrent polls per token and per client IP [guess].
pub const FEED_POLLS: (u8, u16) = (2, 50);
/// Polls held at once per member [guess].
pub const FEED_HELD_MAX: u32 = 1_000;
/// Events kept, and how long: whichever is smaller.
pub const FEED_RING: (usize, Duration) = (10_000, Duration::from_secs(7 * 86_400));
/// `paths` per poll.
pub const FEED_PATHS_MAX: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LeaseChange {
    Acquired,
    Released,
    Expired,
}

impl LeaseChange {
    pub fn as_str(self) -> &'static str {
        match self {
            LeaseChange::Acquired => "acquired",
            LeaseChange::Released => "released",
            LeaseChange::Expired => "expired",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PrAction {
    Opened,
    Readied,
    Closed,
    Merged,
}

impl PrAction {
    pub fn as_str(self) -> &'static str {
        match self {
            PrAction::Opened => "opened",
            PrAction::Readied => "readied",
            PrAction::Closed => "closed",
            PrAction::Merged => "merged",
        }
    }
}

/// One feed event, with its free text raw: the API renders `intent` and `title` through the trust view. `Verdict` is
/// AM4's (`InsightVerdict`) and has no source yet.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ActivityEvent {
    Lease { lease: Lease, change: LeaseChange },
    Task { task: SessionId, state: TaskApiState },
    AgentPush { refname: RefName, head: ObjectId, paths: SmallVec<[Box<str>; 8]> },
    AgentPr { pr: IssueNumber, action: PrAction, title: Box<str>, author: Option<graftyard_types::AccountId> },
}

impl ActivityEvent {
    /// The form a serving member sends the edge, which renders it (`graftyard_api::leases::event_json`).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    pub fn from_json(v: &serde_json::Value) -> Option<ActivityEvent> {
        serde_json::from_value(v.clone()).ok()
    }

    /// The paths the event names, for a poll's `paths` filter: a lease's globs, an agent push's paths. `None` for an
    /// event with no paths (a task or a pull request), which a filtered poll never returns.
    pub fn paths(&self) -> Option<Vec<&str>> {
        match self {
            ActivityEvent::Lease { lease, .. } => Some(lease.scope.paths.iter().map(|g| g.as_str()).collect()),
            ActivityEvent::AgentPush { paths, .. } => Some(paths.iter().map(|p| &**p).collect()),
            _ => None,
        }
    }

    /// Whether a poll with `filter` (empty for none) returns this event.
    pub fn matches(&self, filter: &[&str]) -> bool {
        if filter.is_empty() {
            return true;
        }
        self.paths()
            .is_some_and(|ps| ps.iter().any(|p| filter.iter().any(|f| graftyard_identity::glob::glob_overlap(p, f))))
    }
}

// ---- the cursor ----

/// `?cursor=`: base64url without padding of 17 bytes, a version byte (1), the group's 8-byte `RepoId`, and the 8-byte
/// big-endian index of the last entry the client has seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeedCursor {
    pub group: RepoId,
    pub index: Index,
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

impl FeedCursor {
    pub fn encode(self) -> String {
        let mut b = Vec::with_capacity(17);
        b.push(1u8);
        b.extend_from_slice(&self.group.0.0.to_be_bytes());
        b.extend_from_slice(&self.index.0.to_be_bytes());
        let mut out = String::with_capacity(23);
        for c in b.chunks(3) {
            let n = c.iter().enumerate().fold(0u32, |n, (i, x)| n | u32::from(*x) << (16 - 8 * i));
            for i in 0..=c.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            }
        }
        out
    }

    /// `None` for anything that is not 17 bytes of version 1.
    pub fn decode(s: &str) -> Option<FeedCursor> {
        if s.len() != 23 {
            return None;
        }
        let vals: Option<Vec<u32>> = s.bytes().map(|c| B64.iter().position(|x| *x == c).map(|p| p as u32)).collect();
        let vals = vals?;
        let mut b = Vec::with_capacity(17);
        for c in vals.chunks(4) {
            let n = c.iter().enumerate().fold(0u32, |n, (i, x)| n | x << (18 - 6 * i));
            for i in 0..c.len() - 1 {
                b.push((n >> (16 - 8 * i)) as u8);
            }
        }
        if b.len() != 17 || b[0] != 1 {
            return None;
        }
        let group = i64::from_be_bytes(b[1..9].try_into().ok()?);
        let index = u64::from_be_bytes(b[9..17].try_into().ok()?);
        // The canonical spelling only, so one cursor has one string.
        let c = FeedCursor { group: RepoId(graftyard_types::DbId(group)), index: Index(index) };
        (c.encode() == s).then_some(c)
    }
}

// ---- the query ----

/// A poll's query, parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedQuery {
    pub cursor: Option<String>,
    /// Whole seconds, clamped to FEED_WAIT_MAX.
    pub wait: Duration,
    pub paths: Vec<String>,
}

/// Why a query is refused: 422 with the field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryErr {
    Wait,
    Paths,
    TooManyPaths,
}

fn unescape(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let h = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
                out.push(u8::from_str_radix(h, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// `?cursor=&wait=&paths=` with `paths` repeated. `wait` is whole seconds, 0 by default, clamped to FEED_WAIT_MAX;
/// more than FEED_PATHS_MAX `paths`, or one that is not a `PathGlob`, is refused.
pub fn parse_query(query: &str) -> Result<FeedQuery, QueryErr> {
    let mut q = FeedQuery { cursor: None, wait: Duration::ZERO, paths: Vec::new() };
    for (k, v) in query.split('&').filter(|kv| !kv.is_empty()).map(|kv| kv.split_once('=').unwrap_or((kv, ""))) {
        match k {
            "cursor" => q.cursor = Some(unescape(v).unwrap_or_default()),
            "wait" => {
                let s: u64 = v.parse().map_err(|_| QueryErr::Wait)?;
                q.wait = Duration::from_secs(s).min(FEED_WAIT_MAX);
            }
            "paths" | "paths[]" | "paths%5B%5D" => {
                let p = unescape(v).ok_or(QueryErr::Paths)?;
                crate::agent::PathGlob::parse(&p).map_err(|_| QueryErr::Paths)?;
                q.paths.push(p);
            }
            _ => {}
        }
    }
    if q.paths.len() > FEED_PATHS_MAX {
        return Err(QueryErr::TooManyPaths);
    }
    Ok(q)
}

/// Where a poll starts reading, or why it cannot: a cursor of another group, one older than the ring (or from before
/// a whole-install restore), or a malformed one all get 410 with the oldest cursor the member can serve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    /// Read the entries after this index.
    After(Index),
    /// 410 `{message, oldest_cursor}`.
    Gone { oldest: FeedCursor },
}

/// `oldest` is the index before the first entry the member still holds within FEED_RING; `newest` its last applied.
pub fn start(group: RepoId, cursor: Option<&str>, oldest: Index, newest: Index) -> Start {
    let gone = Start::Gone { oldest: FeedCursor { group, index: oldest } };
    let Some(c) = cursor else { return Start::After(newest) };
    match FeedCursor::decode(c) {
        Some(c) if c.group == group && c.index >= oldest && c.index <= newest => Start::After(c.index),
        _ => gone,
    }
}

/// The oldest index FEED_RING keeps, from the entries the member holds, newest last: the earlier of the last
/// FEED_RING.0 entries and those within FEED_RING.1 of the newest entry's `at`, as the index before the first kept.
pub fn ring_floor(held: &[(Index, Timestamp)]) -> Index {
    let Some(&(_, newest)) = held.last() else { return Index(0) };
    let keep_from = held.len().saturating_sub(FEED_RING.0);
    let window = newest.0.saturating_sub(FEED_RING.1.as_millis() as u64);
    let first = held.iter().enumerate().skip(keep_from).find(|(_, (_, at))| at.0 >= window).map(|(i, _)| i);
    match first {
        Some(i) => Index(held[i].0.0.saturating_sub(1)),
        None => held.last().map_or(Index(0), |(i, _)| *i),
    }
}

// ---- events from one entry ----

/// The lease rows held at `at`, by id.
fn held(v: &LeaseView<'_>) -> std::collections::BTreeMap<LeaseId, LeaseRow> {
    v.live().into_iter().map(|r| (r.lease.id, r)).collect()
}

/// The feed events of one committed entry, from the core and metadata tiers before and after it and its events, read
/// with the owner's `Authz` the member last read. A lease held before and not after is `Released` when the entry
/// released it, or its task, and `Expired` when the entry's `at` passed its `expires` or its holder lost access.
#[allow(clippy::too_many_arguments)]
pub fn events(
    repo: RepoId,
    owner: &dyn AuthzRead,
    before: (&dyn Read, Timestamp),
    after: (&dyn Read, &dyn Read, Timestamp),
    evs: &[RepoEvent],
) -> Vec<ActivityEvent> {
    let bv = LeaseView { core: before.0, owner, repo, at: before.1 };
    let av = LeaseView { core: after.0, owner, repo, at: after.2 };
    let (was, now) = (held(&bv), held(&av));
    let released: Vec<LeaseId> = evs
        .iter()
        .flat_map(|e| match e {
            RepoEvent::LeaseRelease { ids, .. } => ids.to_vec(),
            _ => Vec::new(),
        })
        .collect();
    let mut out = Vec::new();
    for e in evs {
        match e {
            RepoEvent::LeaseAcquire { lease, .. } if now.contains_key(&lease.id) => {
                out.push(ActivityEvent::Lease { lease: lease.clone(), change: LeaseChange::Acquired })
            }
            RepoEvent::TaskCreated(t) => out.push(ActivityEvent::Task { task: t.session.clone(), state: t.state }),
            RepoEvent::TaskUpdated { session, state: Some(s), .. } => {
                out.push(ActivityEvent::Task { task: session.clone(), state: *s })
            }
            RepoEvent::PathSet(p) => out.push(ActivityEvent::AgentPush {
                refname: p.refname.clone(),
                head: p.head,
                paths: p.paths.iter().take(8).cloned().collect(),
            }),
            RepoEvent::AgentMeta { n, .. } => {
                let title =
                    crate::tiers::get_rec::<crate::tiers::IssueRow>(after.1, "issue", &[crate::tiers::n_key(*n)]);
                let (title, author) = title.map_or((Box::from(""), None), |r| (r.title, r.author));
                out.push(ActivityEvent::AgentPr { pr: *n, action: PrAction::Opened, title, author });
            }
            _ => {}
        }
    }
    // Agent pull requests readied, closed, or merged in this entry.
    for e in evs {
        let (n, action) = match e {
            RepoEvent::IssueChanged { n, change: IssueChange::Draft(false), .. } => (*n, PrAction::Readied),
            RepoEvent::PrClosed { n, .. } => (*n, PrAction::Closed),
            RepoEvent::Merged { n, .. } => (*n, PrAction::Merged),
            _ => continue,
        };
        if crate::tiers::agent_meta(after.0, n).is_none() {
            continue;
        }
        let row = crate::tiers::get_rec::<crate::tiers::IssueRow>(after.1, "issue", &[crate::tiers::n_key(n)]);
        let (title, author) = row.map_or((Box::from(""), None), |r| (r.title, r.author));
        out.push(ActivityEvent::AgentPr { pr: n, action, title, author });
    }
    for (id, r) in &was {
        if now.contains_key(id) {
            continue;
        }
        let task_final = r.lease.holder.session.as_ref().is_some_and(|s| {
            evs.iter().any(|e| matches!(e, RepoEvent::TaskUpdated { session, state: Some(st), .. } if session == s && st.is_final()))
        });
        let change = if released.contains(id) || task_final { LeaseChange::Released } else { LeaseChange::Expired };
        let mut lease = r.lease.clone();
        // An expired lease's row may be gone: report it as it last stood.
        if let Some(cur) = crate::lease::lease_row(after.0, *id) {
            lease = cur.lease;
        }
        out.push(ActivityEvent::Lease { lease, change });
    }
    out
}

// ---- a serving member's ring ----

/// A serving member's feed (`design/AGENT-LAYER.md`, "Activity feed": "A serving member answers from its applied log
/// with no read-index call"): the events of the entries it applied, newest last, at most FEED_RING. The host feeds it
/// each applied entry in order with the core tier as it stands after the input that applied it, so events of entries
/// applied in one input read that input's final rows; a lease acquired and released in one input still shows both.
/// `live` is the lease set held at the last entry, which finds `Released` and `Expired`: a lease held before an entry
/// and not after it is `Expired` at the first entry whose `at` passes its `expires`, or whose holder lost access.
#[derive(Clone, Debug, Default)]
pub struct Ring {
    pub events: std::collections::VecDeque<(Index, Timestamp, ActivityEvent)>,
    /// The index and `at` of each entry the ring holds, for `ring_floor`.
    pub held: std::collections::VecDeque<(Index, Timestamp)>,
    /// The last index fed, and the first: cursors before `first` are older than this member's ring.
    pub through: Index,
    pub first: Option<Index>,
    live: std::collections::BTreeMap<LeaseId, Lease>,
}

impl Ring {
    /// The oldest index a cursor may name (the index before the first entry kept), and the newest.
    pub fn bounds(&self) -> (Index, Index) {
        let h: Vec<(Index, Timestamp)> = self.held.iter().copied().collect();
        let floor = ring_floor(&h).max(self.first.map_or(self.through, |f| Index(f.0.saturating_sub(1))));
        (floor, self.through)
    }

    /// Feeds one applied entry (its index, `at`, and events; an entry with none still moves `through` and may expire
    /// leases). `core` and `meta` are the tiers after the input that applied it.
    pub fn feed(
        &mut self,
        repo: RepoId,
        owner: &dyn AuthzRead,
        core: &dyn Read,
        meta: &dyn Read,
        (index, at, evs): (Index, Timestamp, &[RepoEvent]),
    ) {
        if index <= self.through && self.first.is_some() {
            return;
        }
        let view = LeaseView { core, owner, repo, at };
        let now: std::collections::BTreeMap<LeaseId, Lease> =
            view.live().into_iter().map(|r| (r.lease.id, r.lease)).collect();
        // An index past `through + 1` means the entries between never reached the ring (an `InstallSnapshot` moved
        // the member's base past them). The ring starts again after this entry, which only sets the live lease set:
        // a cursor from before gets `gone` rather than a poll that skips the hole, and no lease that went in the hole
        // reads as expired here.
        if self.first.is_some() && index.0 > self.through.0 + 1 {
            self.events.clear();
            self.held.clear();
            self.live = now;
            self.through = index;
            self.first = Some(Index(index.0 + 1));
            return;
        }
        self.first.get_or_insert(index);
        let mut out = Vec::new();
        let released: Vec<LeaseId> = evs
            .iter()
            .flat_map(|e| match e {
                RepoEvent::LeaseRelease { ids, .. } => ids.to_vec(),
                _ => Vec::new(),
            })
            .collect();
        for e in evs {
            match e {
                RepoEvent::LeaseAcquire { lease, .. } => {
                    out.push(ActivityEvent::Lease { lease: lease.clone(), change: LeaseChange::Acquired })
                }
                RepoEvent::TaskCreated(t) => out.push(ActivityEvent::Task { task: t.session.clone(), state: t.state }),
                RepoEvent::TaskUpdated { session, state: Some(st), .. } => {
                    out.push(ActivityEvent::Task { task: session.clone(), state: *st })
                }
                RepoEvent::PathSet(p) => out.push(ActivityEvent::AgentPush {
                    refname: p.refname.clone(),
                    head: p.head,
                    paths: p.paths.iter().take(8).cloned().collect(),
                }),
                RepoEvent::AgentMeta { n, .. } => {
                    let row =
                        crate::tiers::get_rec::<crate::tiers::IssueRow>(meta, "issue", &[crate::tiers::n_key(*n)]);
                    let (title, author) = row.map_or((Box::from(""), None), |r| (r.title, r.author));
                    out.push(ActivityEvent::AgentPr { pr: *n, action: PrAction::Opened, title, author });
                }
                RepoEvent::IssueChanged { n, change: IssueChange::Draft(false), .. }
                | RepoEvent::PrClosed { n, .. }
                | RepoEvent::Merged { n, .. }
                    if crate::tiers::agent_meta(core, *n).is_some() =>
                {
                    let action = match e {
                        RepoEvent::PrClosed { .. } => PrAction::Closed,
                        RepoEvent::Merged { .. } => PrAction::Merged,
                        _ => PrAction::Readied,
                    };
                    let row =
                        crate::tiers::get_rec::<crate::tiers::IssueRow>(meta, "issue", &[crate::tiers::n_key(*n)]);
                    let (title, author) = row.map_or((Box::from(""), None), |r| (r.title, r.author));
                    out.push(ActivityEvent::AgentPr { pr: *n, action, title, author });
                }
                _ => {}
            }
        }
        // `core` is the tier as the whole input left it, and a follower's catch-up applies many entries in one input.
        // A lease whose row is gone with nothing in this entry ending it (no release, no final task, not yet expired)
        // was ended by a later entry of the same input: it stays live until that entry, which reports it.
        let ended_later =
            |l: &Lease| crate::lease::lease_row(core, l.id).is_none() && !released.contains(&l.id) && at < l.expires;
        let mut now = now;
        for (id, l) in &self.live {
            if now.contains_key(id) {
                continue;
            }
            let task_final = l.holder.session.as_ref().is_some_and(|s| {
                evs.iter().any(|e| matches!(e, RepoEvent::TaskUpdated { session, state: Some(st), .. } if session == s && st.is_final()))
            });
            if !task_final && ended_later(l) {
                now.insert(*id, l.clone());
                continue;
            }
            let change = if released.contains(id) || task_final { LeaseChange::Released } else { LeaseChange::Expired };
            out.push(ActivityEvent::Lease { lease: l.clone(), change });
        }
        for e in evs {
            if let RepoEvent::LeaseAcquire { lease, .. } = e
                && !now.contains_key(&lease.id)
                && ended_later(lease)
            {
                now.insert(lease.id, lease.clone());
            }
        }
        self.live = now;
        self.through = index;
        self.held.push_back((index, at));
        self.events.extend(out.into_iter().map(|e| (index, at, e)));
        // FEED_RING: the last FEED_RING.0 entries and those within FEED_RING.1 of the newest.
        let (floor, _) = self.bounds();
        while self.held.front().is_some_and(|(i, _)| *i <= floor) {
            self.held.pop_front();
        }
        while self.events.front().is_some_and(|(i, _, _)| *i <= floor) {
            self.events.pop_front();
        }
    }

    /// The member's base moved to `to` with no entry fed (an `InstallSnapshot` applied through `to`). The ring
    /// restarts there at once: a cursor from before `to` gets `gone` now, not only once the next entry arrives, which
    /// on a quiet group can be long after the member follows again (LF-13 on graft-m3c: a `hold` from the pre-fault
    /// cursor skipped 19 and 11 events). The live lease set becomes the one `core`, the installed tier at `to`
    /// with entry time `at`, holds, so a lease held there and released by the next entry still reports `Released`.
    pub fn skip(&mut self, repo: RepoId, owner: &dyn AuthzRead, core: &dyn Read, (to, at): (Index, Timestamp)) {
        if self.first.is_some() && to <= self.through {
            return;
        }
        let view = LeaseView { core, owner, repo, at };
        self.events.clear();
        self.held.clear();
        self.live = view.live().into_iter().map(|r| (r.lease.id, r.lease)).collect();
        self.through = to;
        self.first = Some(Index(to.0 + 1));
    }

    /// Entries a member applied while it could not feed them (its owner's `Authz` was not read yet), once it can: a
    /// ring that never served starts after the last of them with its live leases, as `skip` does for an install, so a
    /// cursor from before gets 410; a ring that already serves feeds each, so none a cursor could ask for is skipped.
    /// Fed one by one, each entry rebuilt the live-lease set from SQL in one synchronous turn: each of L0-12's 8
    /// learners spent 15 to 46 s of CPU on its first poll with about 1,070 entries and 500 leases waiting, and the
    /// voters sharing their isolates answered "Requests queued for too long" (LM-1 round 7).
    pub fn backfill(
        &mut self,
        repo: RepoId,
        owner: &dyn AuthzRead,
        core: &dyn Read,
        meta: &dyn Read,
        pending: Vec<(Index, Timestamp, Vec<RepoEvent>)>,
    ) {
        if self.first.is_none() {
            if let Some((i, at, _)) = pending.last() {
                self.skip(repo, owner, core, (*i, *at));
            }
            return;
        }
        for (i, at, evs) in pending {
            self.feed(repo, owner, core, meta, (i, at, &evs));
        }
    }

    /// The events past `after` that a poll with `filter` returns.
    pub fn past(&self, after: Index, filter: &[&str]) -> Vec<(Index, Timestamp, ActivityEvent)> {
        self.events.iter().filter(|(i, _, e)| *i > after && e.matches(filter)).cloned().collect()
    }
}

/// How many learners the feed needs for a group (`design/AGENT-LAYER.md`, "Activity feed": "`place_learners` adds
/// learners until held plus refused fit in half of FEED_HELD_MAX each"), from each serving member's reported peak held
/// count and refusals: the polls all of them carried, over half of FEED_HELD_MAX, rounded up.
pub fn learners_for_feed(reports: &[(u32, u32)]) -> usize {
    let total: u64 = reports.iter().map(|(peak, refused)| u64::from(*peak) + u64::from(*refused)).sum();
    total.div_ceil(u64::from(FEED_HELD_MAX / 2)) as usize
}

// ---- held polls ----

/// What a serving member knows of its own replication (`design/AGENT-LAYER.md`, "Activity feed"): whether its last
/// AppendEntries left a gap, whether an `InstallSnapshot` catch-up is running, and how far its last applied entry's
/// `at` trails the leader's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Following {
    pub gap: bool,
    pub installing: bool,
    pub lag: Duration,
}

impl Following {
    /// Following: no gap, no install, and applied within FEED_BATCH of the leader's `at`.
    pub fn following(self) -> bool {
        !self.gap && !self.installing && self.lag <= FEED_BATCH
    }
}

/// Which members may serve: learners first, followers only while no learner answers, never the leader or a witness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedRole {
    Leader,
    Follower,
    Learner,
    Witness,
}

/// What a member answers a new poll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admit {
    /// Hold it (or answer at once if events are already past the cursor).
    Hold,
    /// It is lagging: answer the cursor and no events; the edge tries another member.
    Lagging,
    /// At FEED_HELD_MAX: the edge answers 429 with `Retry-After` at this first full reply.
    Full,
    /// Not a serving member (the leader, a witness, or a follower while a learner answers).
    NotServing,
}

/// A serving member's held polls: their count and its peak, which it reports for `place_learners`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Serving {
    pub held: u32,
    pub peak: u32,
    pub refused: u32,
}

impl Serving {
    pub fn admit(&mut self, role: FeedRole, learner_answers: bool, f: Following) -> Admit {
        let serves = match role {
            FeedRole::Learner => true,
            FeedRole::Follower => !learner_answers,
            FeedRole::Leader | FeedRole::Witness => false,
        };
        if !serves {
            return Admit::NotServing;
        }
        if !f.following() {
            return Admit::Lagging;
        }
        if self.held >= FEED_HELD_MAX {
            self.refused += 1;
            return Admit::Full;
        }
        self.held += 1;
        self.peak = self.peak.max(self.held);
        Admit::Hold
    }

    /// A held poll answered.
    pub fn done(&mut self) {
        self.held = self.held.saturating_sub(1);
    }

    /// The member stopped following: every held poll is answered at once with its cursor and no events. Returns how
    /// many.
    pub fn drop_all(&mut self) -> u32 {
        std::mem::take(&mut self.held)
    }
}

/// The edge's choice for one poll after asking members in order (at most three): the first that holds it; else, if
/// every one tried refused as lagging and the least lagged is a learner, that learner; else 429 with `Retry-After` of
/// FEED_BATCH_ALL. A full member gets the 429 at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Held(usize),
    TooMany { retry_after: Duration },
}

pub fn route(answers: &[(Admit, FeedRole, Duration)]) -> Route {
    let retry = Route::TooMany { retry_after: FEED_BATCH_ALL };
    for (i, (a, _, _)) in answers.iter().enumerate().take(3) {
        match a {
            Admit::Hold => return Route::Held(i),
            Admit::Full => return retry,
            Admit::Lagging | Admit::NotServing => {}
        }
    }
    let lagging: Vec<(usize, FeedRole, Duration)> = answers
        .iter()
        .enumerate()
        .take(3)
        .filter(|(_, (a, _, _))| *a == Admit::Lagging)
        .map(|(i, (_, r, l))| (i, *r, *l))
        .collect();
    let all_lagging = !lagging.is_empty() && lagging.len() == answers.len().min(3);
    match lagging.iter().min_by_key(|(_, _, l)| *l) {
        Some((i, FeedRole::Learner, _)) if all_lagging => Route::Held(*i),
        _ => retry,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graftyard_types::DbId;

    #[test]
    fn a_cursor_is_17_bytes_in_base64url() {
        let c = FeedCursor { group: RepoId(DbId(7)), index: Index(0x0102_0304) };
        let s = c.encode();
        assert_eq!(s.len(), 23);
        assert!(!s.contains('=') && !s.contains('+') && !s.contains('/'));
        assert_eq!(FeedCursor::decode(&s), Some(c));
        assert_eq!(FeedCursor::decode("AQAAAAAAAAAHAAAAAAECAwQ"), Some(c));
        assert_eq!(FeedCursor::decode("x"), None);
        let mut bad = s.clone().into_bytes();
        bad[1] = b'A';
        assert_eq!(FeedCursor::decode(std::str::from_utf8(&bad).unwrap()), None, "version 0");
    }

    #[test]
    fn the_query_clamps_wait_and_caps_paths() {
        let q = parse_query("wait=40&paths=pkg%2Fauth%2F**&paths=docs/*").unwrap();
        assert_eq!(q.wait, FEED_WAIT_MAX);
        assert_eq!(q.paths, ["pkg/auth/**", "docs/*"]);
        let many: String = (0..33).map(|i| format!("paths=p{i}&")).collect();
        assert_eq!(parse_query(&many), Err(QueryErr::TooManyPaths));
        assert_eq!(parse_query("wait=x"), Err(QueryErr::Wait));
        assert_eq!(parse_query("").unwrap().wait, Duration::ZERO);
    }

    #[test]
    fn learners_serve_and_full_or_lagging_members_send_polls_elsewhere() {
        let ok = Following { gap: false, installing: false, lag: Duration::from_millis(200) };
        let mut s = Serving::default();
        assert_eq!(s.admit(FeedRole::Leader, false, ok), Admit::NotServing);
        assert_eq!(s.admit(FeedRole::Follower, true, ok), Admit::NotServing);
        assert_eq!(s.admit(FeedRole::Follower, false, Following { gap: true, ..ok }), Admit::Lagging);
        for _ in 0..FEED_HELD_MAX {
            assert_eq!(s.admit(FeedRole::Learner, false, ok), Admit::Hold);
        }
        assert_eq!(s.admit(FeedRole::Learner, false, ok), Admit::Full);
        assert_eq!((s.peak, s.drop_all(), s.held), (FEED_HELD_MAX, FEED_HELD_MAX, 0));
        let lag = |r, ms| (Admit::Lagging, r, Duration::from_millis(ms));
        assert_eq!(route(&[lag(FeedRole::Follower, 3_000), lag(FeedRole::Learner, 1_500)]), Route::Held(1));
        let full = Route::TooMany { retry_after: FEED_BATCH_ALL };
        assert_eq!(route(&[lag(FeedRole::Learner, 3_000), lag(FeedRole::Follower, 1_500)]), full);
        assert_eq!(route(&[(Admit::Full, FeedRole::Learner, Duration::ZERO)]), full);
    }
}
