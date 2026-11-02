//! The edge's side of one write (`design/DESIGN.md`, "Commit and read paths", and Shape, "The write path"): a hedge
//! on a missed receipt is a `suspect` report to a follower on another site, never the command forwarded to a second
//! member (BF-14).
//!
//! A `Receipt` owns the command and the one member it may be sent to. `missed` returns `Suspect` reports, which carry
//! no command, and cannot change the target. The target changes only on an answer from a member: a `NotLeader`, or a
//! `suspect` verdict of `Leader` from the member that now leads.

use graftyard_types::SiteId;
use smallvec::SmallVec;

use crate::member::Verdict;
use crate::quorum;
use crate::types::{Colo, GroupConfig, NotLeader, RequestId, Term};

/// One `suspect` report: call `Member::suspect(now, from, about, term)` on member `to`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Suspect {
    pub to: SiteId,
    pub from: Colo,
    /// The member whose receipt missed, or that answered `NotLeader` naming no leader.
    pub about: SiteId,
    pub term: Term,
}

/// One client request's command at the edge.
#[derive(Clone, Debug)]
pub struct Receipt<C> {
    id: RequestId,
    cmd: C,
    target: SiteId,
    /// The highest term an answer named.
    term: Term,
    colo: Colo,
}

impl<C> Receipt<C> {
    pub fn new(id: RequestId, cmd: C, target: SiteId, colo: Colo) -> Receipt<C> {
        Receipt { id, cmd, target, term: Term(0), colo }
    }

    pub fn id(&self) -> RequestId {
        self.id
    }

    pub fn target(&self) -> SiteId {
        self.target
    }

    /// What to send, and the only member to send it to: `Member::commit(now, id, cmd)` on `target`.
    pub fn call(&self) -> (SiteId, RequestId, &C) {
        (self.target, self.id, &self.cmd)
    }

    /// The receipt from `target` missed CALL_DEADLINE, or the call failed. The hedge: a `suspect` report to every
    /// other voter of `config`. The target is unchanged, so the next send goes to the same member.
    pub fn missed(&self, config: &GroupConfig) -> SmallVec<[Suspect; 2]> {
        self.report(config, self.target)
    }

    /// `target` answered `NotLeader`, so it holds no ticket for the command. A named leader becomes the target. With
    /// no leader named, the edge reports to every voter, `target` included, which may hold the newest log after a
    /// reset deposed it, and moves to the next voter after `target`.
    pub fn not_leader(&mut self, nl: &NotLeader, config: &GroupConfig) -> SmallVec<[Suspect; 2]> {
        self.term = self.term.max(nl.term);
        if let Some(l) = nl.leader
            && l != self.target
        {
            self.target = l;
            return SmallVec::new();
        }
        let voters: Vec<SiteId> = quorum::voters(config).into_iter().collect();
        let about = self.target;
        let out = voters.iter().map(|s| Suspect { to: *s, from: self.colo, about, term: self.term }).collect();
        if let Some(i) = voters.iter().position(|s| *s == self.target) {
            self.target = voters[(i + 1) % voters.len()];
        }
        out
    }

    /// A `suspect` report to `from` answered `v`. A member that leads becomes the target.
    pub fn verdict(&mut self, from: SiteId, v: Verdict) {
        if matches!(v, Verdict::Leader | Verdict::Won | Verdict::Resumed) {
            self.target = from;
        }
    }

    /// A reply from any member carried a term.
    pub fn saw_term(&mut self, t: Term) {
        self.term = self.term.max(t);
    }

    fn report(&self, config: &GroupConfig, about: SiteId) -> SmallVec<[Suspect; 2]> {
        quorum::voters(config)
            .into_iter()
            .filter(|s| *s != about)
            .map(|s| Suspect { to: s, from: self.colo, about, term: self.term })
            .collect()
    }
}
