//! Snapshot install and reads off the leader (`design/DESIGN.md`, Shape, "Packs", `InstallSnapshot`, and "The read
//! path").
//!
//! - **Install.** A leader whose host no longer stores the entries at or below some index names, with `set_install`,
//!   the snapshot and archive objects that hold them. A member whose next entry is at or below that index gets an
//!   `InstallSnapshot` in place of AppendEntries. Its host loads the objects into a fold at `through` and calls
//!   `Member::install`, which drops the member's log and starts it above `through`; only then does the member answer
//!   the round, so it acks no AppendEntries before it has applied `through`. A learner and a `ReplaceMember` member join
//!   the same way.
//! - **Read index.** A follower or learner answers a read by asking the leader for a read index and waiting until it
//!   has applied it. Reads are batched: one batch in flight, sent at most every `FOLLOWER_READ_EVERY` (10 ms) from a
//!   follower and `LEARNER_READ_EVERY` (50 ms) from a learner.

use std::collections::BTreeMap;
use std::time::Duration;

use graftyard_types::{SiteId, Timestamp, learner_slot};

use super::election::CAMPAIGN_DEADLINE;
use super::{
    AppendRes, InstallError, InstallSnapshot, InstallSource, InstallWanted, LogBase, Member, Message, ReadId,
    ReadStart, Ready, Resolution, Role, Write,
};
use crate::fold::{Command, GroupFold};
use crate::read::ReadGrant;
use crate::types::{Digest, Index, NotLeader};
use crate::{CALL_DEADLINE, after};

/// A follower's read-index batches: at most one every 10 ms.
const FOLLOWER_READ_EVERY: Duration = Duration::from_millis(10);
/// A learner's: one request in flight, at most one every 50 ms.
pub const LEARNER_READ_EVERY: Duration = Duration::from_millis(50);

/// Reads by id, with when each arrived.
type Arrived = Vec<(ReadId, Timestamp)>;

/// Reads waiting on the leader's read index, and the leader's side of others' batches.
#[derive(Default)]
pub(super) struct RemoteReads {
    /// Not yet in a batch: id and arrival.
    queued: Arrived,
    /// The batch in flight: its number, when it was last sent, and its reads.
    inflight: Option<(u64, Timestamp, Arrived)>,
    /// Answered: each read's index, waiting until this member has applied it.
    waiting: Vec<(ReadId, Timestamp, Index)>,
    next_batch: u64,
    last_sent: Option<Timestamp>,
    /// As leader: the reads started for another member's batch, by the id `start_read` gave them.
    serving: BTreeMap<ReadId, (SiteId, u64)>,
}

impl<C: Command> Member<C> {
    /// Whether this member is a learner: it never votes, campaigns, or leads.
    pub(super) fn is_learner(&self) -> bool {
        learner_slot(self.cfg.me).is_some()
    }

    /// The leader's host no longer stores the entries at or below `source.through`: a member whose next entry is
    /// there gets `source` as an `InstallSnapshot`. `None` when the host stores the whole log again.
    pub fn set_install(&mut self, source: Option<InstallSource>) {
        self.install = source;
    }

    /// `send_append`'s check: the follower's next entry is at or below the archive boundary, so it is sent the
    /// install. True if it was.
    pub(super) fn send_install(&mut self, peer: SiteId, prev: Index, round: u64) -> bool {
        let Some(source) = self.install.clone() else { return false };
        if prev >= source.through {
            return false;
        }
        let req = InstallSnapshot {
            term: self.state.term,
            boot: self.boot,
            round,
            sent_at: self.now,
            lead_seq: self.state.lead_seq,
            source,
        };
        self.send(peer, Message::Install(req));
        true
    }

    fn install_res(&self, req: &InstallSnapshot, ok: bool, matched: Index) -> AppendRes {
        AppendRes {
            term: self.state.term,
            boot: req.boot,
            round: req.round,
            sent_at: req.sent_at,
            prev_index: req.source.through,
            ok,
            matched,
            last: self.last(),
            promise: false,
            refused: None,
            stepped_down_at: self.state.stepped_down_at,
            boots: self.state.boots,
            release: self.cfg.release,
        }
    }

    /// Holds `through` already: committed, or the same entry in its log, which chains everything below.
    fn holds(&self, s: &InstallSource) -> bool {
        self.state.commit >= s.through || self.digest_at(s.through) == Some(s.through_digest)
    }

    pub(super) fn on_install(&mut self, from: SiteId, req: InstallSnapshot) {
        if req.term < self.state.term || (req.term == self.state.term && req.lead_seq < self.state.follow_seq) {
            let res = self.install_res(&req, false, Index(0));
            self.send(from, Message::AppendRes(res));
            return;
        }
        if req.term > self.state.term || self.role != Role::Follower {
            self.step_down(req.term);
        }
        if req.lead_seq > self.state.follow_seq {
            self.state.follow_seq = req.lead_seq;
            self.dirty = true;
        }
        self.leader = Some(from);
        self.last_leader_seen = self.now;
        if self.holds(&req.source) {
            let res = self.install_res(&req, true, req.source.through);
            self.send(from, Message::AppendRes(res));
            return;
        }
        self.out.install.push(InstallWanted { from, install: req });
    }

    /// The host loaded `wanted`'s snapshot and archives into `fold`, at `through`, with the request rows and the
    /// committed config and id block as of there. The member drops its log, which holds nothing committed past what
    /// it loaded, and continues above `through`; it then answers the install's round. The host's own storage holds
    /// `fold` by the time `Write::Install` is durable.
    pub fn install(
        &mut self,
        now: Timestamp,
        wanted: InstallWanted,
        fold: GroupFold<C::Fold>,
    ) -> Ready<C, Result<(), InstallError>> {
        self.begin(now);
        let r = self.take_install(wanted, fold);
        self.finish(r)
    }

    fn take_install(&mut self, wanted: InstallWanted, fold: GroupFold<C::Fold>) -> Result<(), InstallError> {
        let InstallWanted { from, install: req } = wanted;
        let s = &req.source;
        if self.role == Role::Leader || req.term != self.state.term || self.leader != Some(from) {
            return Err(InstallError::NotNeeded);
        }
        if self.holds(s) {
            let res = self.install_res(&req, true, s.through);
            self.send(from, Message::AppendRes(res));
            return Err(InstallError::NotNeeded);
        }
        if fold.applied() != s.through {
            return Err(InstallError::Mismatch { what: "the loaded fold is not at through" });
        }
        if fold.config() != Some(&s.config) || fold.id_block() != s.id_block {
            return Err(InstallError::Mismatch { what: "the loaded config or id block is not the install's" });
        }
        // Everything in the log is either at or below `through`, which the fold replaces, or above an entry at
        // `through` that differs from the committed one, which no quorum can have committed.
        let base = LogBase { index: s.through, term: s.through_term, digest: s.through_digest };
        self.log.clear();
        self.digests.clear();
        self.sizes.clear();
        self.log_bytes = 0;
        self.configs.clear();
        self.epoch += 1;
        self.own_appended = None;
        self.base = base;
        self.fold = fold;
        self.state.commit = s.through;
        self.state.applied = s.through;
        self.own_durable = s.through;
        self.own_durable_at = self.now;
        self.dirty = true;
        self.writes.push(Write::Install(base));
        let res = self.install_res(&req, true, s.through);
        self.send(from, Message::AppendRes(res));
        Ok(())
    }

    /// A read on a follower or learner: answered once this member has applied the leader's read index, as a
    /// `Resolution::Read` whose grant `fresh` turns into the view. On the leader, `read`.
    pub fn read_index(&mut self, now: Timestamp) -> Ready<C, ReadStart> {
        self.begin(now);
        let r = if self.role == Role::Leader { self.start_read() } else { self.queue_remote() };
        self.finish(r)
    }

    fn queue_remote(&mut self) -> ReadStart {
        if self.leader.is_none_or(|l| l == self.cfg.me) {
            return Err(self.not_leader());
        }
        let id = ReadId(self.next_read);
        self.next_read += 1;
        self.remote.queued.push((id, self.now));
        self.send_batch();
        Ok(id)
    }

    fn send_batch(&mut self) {
        let every = if self.is_learner() { LEARNER_READ_EVERY } else { FOLLOWER_READ_EVERY };
        let Some(leader) = self.leader.filter(|l| *l != self.cfg.me) else { return };
        let r = &mut self.remote;
        if r.inflight.is_some() || r.queued.is_empty() || r.last_sent.is_some_and(|t| self.now < after(t, every)) {
            return;
        }
        r.next_batch += 1;
        let batch = r.next_batch;
        r.last_sent = Some(self.now);
        r.inflight = Some((batch, self.now, std::mem::take(&mut r.queued)));
        self.send(leader, Message::ReadIndex { batch });
    }

    /// The leader's side: a read for the asker's batch, granted by the same lease or round as the leader's own.
    pub(super) fn on_read_index(&mut self, from: SiteId, batch: u64) {
        if self.role != Role::Leader {
            self.send(from, Message::ReadIndexRes { batch, index: None });
            return;
        }
        let id = ReadId(self.next_read);
        self.remote.serving.insert(id, (from, batch));
        if self.start_read().is_err() {
            self.remote.serving.remove(&id);
            self.send(from, Message::ReadIndexRes { batch, index: None });
        }
    }

    /// A read the leader resolved: its own, as a `Resolution::Read`, or another member's batch, as a message.
    pub(super) fn read_resolved(&mut self, id: ReadId, result: Result<ReadGrant, NotLeader>) {
        match self.remote.serving.remove(&id) {
            Some((to, batch)) => {
                let index = result.ok().and_then(|g| self.digest_at(g.index).map(|d| (g.index, d)));
                self.send(to, Message::ReadIndexRes { batch, index })
            }
            None => self.out.resolutions.push(Resolution::Read { id, result }),
        }
    }

    pub(super) fn on_read_index_res(&mut self, from: SiteId, batch: u64, index: Option<(Index, Digest)>) {
        if self.remote.inflight.as_ref().is_none_or(|b| b.0 != batch) {
            return;
        }
        let (_, _, reads) = self.remote.inflight.take().expect("checked");
        match index {
            Some((i, d)) => {
                // An idle leader sends no append that would carry its commit. Entry `i` is committed, and digests
                // chain, so if this member's entry `i` has the same digest, everything through it is committed.
                if i > self.state.commit && self.digest_at(i) == Some(d) {
                    self.set_commit(i);
                }
                self.remote.waiting.extend(reads.into_iter().map(|(id, at)| (id, at, i)));
            }
            None => {
                // That member does not lead: the reads fail, and the caller asks the leader it names.
                if self.leader == Some(from) {
                    self.leader = None;
                }
                let nl = self.not_leader();
                for (id, _) in reads {
                    self.out.resolutions.push(Resolution::Read { id, result: Err(nl.clone()) });
                }
            }
        }
        self.check_remote();
        self.send_batch();
    }

    /// Grants every answered read whose index this member has applied, and fails those past CAMPAIGN_DEADLINE.
    pub(super) fn check_remote(&mut self) {
        if self.remote.waiting.is_empty() && self.remote.queued.is_empty() && self.remote.inflight.is_none() {
            return;
        }
        let (applied, now) = (self.fold.applied(), self.now);
        let late = |at: Timestamp| now >= after(at, CAMPAIGN_DEADLINE);
        let nl = self.not_leader();
        let grant = |m: &Self| ReadGrant {
            term: m.state.term,
            boot: m.boot,
            step: m.step,
            index: applied,
            via_round: true,
            remote: true,
        };
        let waiting = std::mem::take(&mut self.remote.waiting);
        for (id, at, i) in waiting {
            if i <= applied {
                let g = grant(self);
                self.out.resolutions.push(Resolution::Read { id, result: Ok(g) });
            } else if late(at) {
                self.out.resolutions.push(Resolution::Read { id, result: Err(nl.clone()) });
            } else {
                self.remote.waiting.push((id, at, i));
            }
        }
        let r = &mut self.remote;
        let mut failed = Vec::new();
        r.queued.retain(|(id, at)| !late(*at) || (failed.push(*id), false).1);
        let unanswered = failed.len();
        if let Some((_, _, reads)) = &mut r.inflight {
            reads.retain(|(id, at)| !late(*at) || (failed.push(*id), false).1);
        }
        // The leader left a read-index batch unanswered for CAMPAIGN_DEADLINE (F67): the design's "read-index call to
        // the leader fails". This member forgets that leader, so its reads answer `NotLeader` naming none, which the
        // edge reports with `suspect`, and a voter whose promise has lapsed campaigns now. Without it a group read
        // only through followers and learners never elects: their reads shed with no report.
        let failed_call = failed.len() > unanswered;
        let nl = if failed_call {
            self.forgot = self.leader.or(self.forgot);
            self.leader = None;
            if self.role == Role::Follower {
                let _ = self.try_campaign();
            }
            self.not_leader()
        } else {
            nl
        };
        for id in failed {
            self.out.resolutions.push(Resolution::Read { id, result: Err(nl.clone()) });
        }
    }

    /// Time passing for remote reads: a batch unanswered after CALL_DEADLINE is sent again, and queued reads go out.
    pub(super) fn tick_remote(&mut self) {
        if let Some((batch, sent, _)) = &mut self.remote.inflight
            && self.now >= after(*sent, CALL_DEADLINE)
        {
            *sent = self.now;
            let batch = *batch;
            if let Some(l) = self.leader.filter(|l| *l != self.cfg.me) {
                self.send(l, Message::ReadIndex { batch });
            }
        }
        self.check_remote();
        self.send_batch();
    }
}
