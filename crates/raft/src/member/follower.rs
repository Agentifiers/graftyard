//! The follower side of AppendEntries: promises, log matching by digest, apply before verify, and refusals.

use graftyard_types::SiteId;

use crate::after;
use crate::fold::Command;
use crate::types::Index;
use crate::{CALL_DEADLINE, ELECTION_AFTER};

use super::log::verify;
use super::{AppendReq, AppendRes, Member, Message, Role};

impl<C: Command> Member<C> {
    pub(super) fn on_append(&mut self, from: SiteId, req: AppendReq<crate::Ev<C>>) {
        let mut res = AppendRes {
            term: self.state.term,
            boot: req.boot,
            round: req.round,
            sent_at: req.sent_at,
            prev_index: req.prev_index,
            ok: false,
            matched: Index(0),
            last: self.last(),
            promise: false,
            refused: None,
            stepped_down_at: self.state.stepped_down_at,
            boots: self.state.boots,
            release: self.cfg.release,
        };
        // An older instance of a leader of this term, or of an older term: refused, and nothing changes.
        if req.term < self.state.term || (req.term == self.state.term && req.lead_seq < self.state.follow_seq) {
            self.send(from, Message::AppendRes(res));
            return;
        }
        // A leader of this term exists, so this member is a follower in it. A member that was itself leader of this
        // term cannot get here: one leader per term.
        if req.term > self.state.term || self.role != Role::Follower {
            debug_assert!(
                !(req.term == self.state.term && self.role == Role::Leader),
                "{:?} and {from:?} both lead term {:?}",
                self.cfg.me,
                req.term
            );
            self.step_down(req.term);
        }
        // A pre-vote ends when the leader is heard from and this member promises it again. While `suspect` withholds
        // promises it keeps running: otherwise every round of a still-renewing gray leader cancels it (found by the
        // liveness runs, slow seed 2023: 9 s of cancelled pre-votes).
        let withholding = self.now < self.withhold_until;
        if self.campaign.as_ref().is_some_and(|c| !c.pre() || !withholding || req.transfer.is_some()) {
            self.campaign = None;
        }
        if req.lead_seq > self.state.follow_seq {
            self.state.follow_seq = req.lead_seq;
            self.dirty = true;
        }
        // Another leader is heard: every hand-off's bar on an earlier leader ends (`handing_off`).
        let ended: Vec<SiteId> = self.handing_off.iter().copied().filter(|h| *h != req.leader).collect();
        for h in ended {
            self.handing_off.remove(&h);
            self.suspected.remove(&h);
        }
        self.leader = Some(req.leader);
        self.forgot = None;
        // A leader two colos reported, from which this member withholds promises, does not count as seen, or its
        // appends, still arriving from a half-hung site, kept the 2 x ELECTION_AFTER waiver from ever freeing two
        // healthy voters that quarantined each other (F67, live seed 11089: 32.9 s).
        if !withholding {
            self.last_leader_seen = self.now;
        }
        res.term = self.state.term;
        // The transfer id of the last AppendEntries accepted, in the leader's send order: a voter disregards its promise
        // only for a transfer vote naming it.
        let transfer = req.transfer.map(|(_, id)| id);
        if req.sent_at >= self.state.transfer_at
            && (self.state.transfer != transfer || self.state.transfer_at != req.sent_at)
        {
            self.state.transfer = transfer;
            self.state.transfer_at = req.sent_at;
            self.dirty = true;
        }
        // A promise, unless the round carries a transfer, whose acks extend no lease, or two colos reported the leader
        // (`suspect`) and the term has not changed since.
        let current = self.state.transfer.is_none() && req.sent_at >= self.state.transfer_at;
        if req.transfer.is_none() && current && self.now >= self.withhold_until {
            let until = after(self.now, ELECTION_AFTER);
            if until > self.state.promise_until || self.state.promised_to != Some(req.leader) {
                self.state.promise_until = self.state.promise_until.max(until);
                self.state.promised_to = Some(req.leader);
                self.dirty = true;
            }
            res.promise = true;
        }
        // Entries match by digest, not term: a restarted leader that resumes `led_term` may have lost an entry it
        // sent before its own write was durable, and send a different entry at the same (term, index). That entry
        // never committed, because a commit is revealed only after the leader's own write is durable.
        // Entries at or below `base` are committed here, so they match the leader's: skip them.
        let mut req = req;
        if req.prev_index < self.base.index {
            let base = self.base.index;
            let (below, above): (Vec<_>, Vec<_>) = req.entries.into_iter().partition(|e| e.index <= base);
            let top = below.last().map_or(req.prev_index, |e| e.index);
            if above.is_empty() {
                self.set_commit(req.commit.min(top));
                res.ok = true;
                res.matched = top;
                res.last = self.last();
                res.term = self.state.term;
                self.send(from, Message::AppendRes(res));
                return;
            }
            req.entries = above;
            req.prev_index = base;
            req.prev_digest = self.base.digest;
        }
        if req.prev_index > self.last() {
            res.last = self.last();
            self.send(from, Message::AppendRes(res));
            return;
        }
        if self.digest_at(req.prev_index) != Some(req.prev_digest) {
            debug_assert!(req.prev_index > self.state.commit, "committed entry differs from the leader's");
            if req.prev_index > self.state.commit {
                self.truncate_from(req.prev_index);
            }
            res.last = self.last();
            self.send(from, Message::AppendRes(res));
            return;
        }
        let mut matched = req.prev_index;
        for e in req.entries {
            if e.index <= self.last() {
                if self.digest_at(e.index) == Some(crate::digest(&e)) {
                    matched = e.index;
                    continue;
                }
                self.truncate_from(e.index);
            }
            // A follower verifies k+1 only after it applies k: the append carries the leader's commit index.
            self.set_commit(req.commit.min(Index(e.index.0 - 1)));
            // A member with no committed config stores only the bootstrap entry.
            if e.index > Index(1) && self.fold.config().is_none() {
                break;
            }
            if let Err(refusal) = verify(&e, &self.view_for(e.index)) {
                res.refused = Some(refusal);
                break;
            }
            self.push_entry(e);
            matched = self.last();
        }
        self.set_commit(req.commit.min(matched));
        // A stall marker: the leader's own entry that no quorum acknowledged within CALL_DEADLINE, which the leader
        // marks too (`tick_leader`), learned from its re-send. Otherwise only the leader knows when a stall began, and
        // a follower that wins the next term would stamp no `extend` for it (L0-12 hosted: a 30-minute stall ended by
        // a leader change moved no lease). Above this member's commit, so a delayed append after the group committed
        // past the entry marks nothing.
        let unacked = self
            .log
            .iter()
            .rev()
            .take_while(|e| e.index > self.state.commit.max(req.commit))
            .filter(|e| e.index <= matched && e.term == req.term && req.sent_at >= after(e.at, CALL_DEADLINE));
        if let Some(at) = unacked.map(|e| e.at).min() {
            self.state.stalled_since = Some(self.state.stalled_since.map_or(at, |m| m.min(at)));
        }
        res.ok = res.refused.is_none();
        res.matched = matched;
        res.last = self.last();
        res.term = self.state.term;
        self.send(from, Message::AppendRes(res));
    }
}
