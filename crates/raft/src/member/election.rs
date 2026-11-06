//! Bootstrap, pre-vote, votes, promises, `RESET_QUARANTINE`, and `led_term` resumption.

use std::collections::BTreeSet;
use std::time::Duration;

use graftyard_types::{SiteId, Timestamp};

use crate::fold::Command;
use crate::quorum;
use crate::types::{Colo, GroupConfig, Index, Term, TransferId};

use crate::{CALL_DEADLINE, COMMIT_RETRIES, ELECTION_AFTER, RESET_QUARANTINE, SUSPECT, after};

use super::{Deny, Member, Message, Ready, Resolution, Role, Verdict, VoteReq, VoteRes};

/// How long a campaign or a bootstrap proposal waits for answers before it counts as lost.
pub(super) const CAMPAIGN_DEADLINE: Duration =
    Duration::from_millis(CALL_DEADLINE.as_millis() as u64 * (1 + COMMIT_RETRIES as u64));

pub(super) struct Campaign {
    pre: bool,
    /// The term asked for.
    term: Term,
    granted: BTreeSet<SiteId>,
    answered: BTreeSet<SiteId>,
    started: Timestamp,
    sent_at: Timestamp,
    /// A campaign a leader's `TimeoutNow` started: no pre-vote, and the vote request names the transfer.
    transfer: Option<TransferId>,
}

impl Campaign {
    pub(super) fn transfer(&self) -> Option<TransferId> {
        self.transfer
    }
    pub(super) fn pre(&self) -> bool {
        self.pre
    }
}

impl<C: Command> Member<C> {
    /// A campaign request with no report: the creating call of a new group (which proposes the bootstrap entry), or a
    /// restarted leader resuming `led_term`. Otherwise it campaigns as `suspect` does once the promise has lapsed.
    /// Campaigns are request-driven, never started by `tick`.
    pub fn campaign(&mut self, now: Timestamp) -> Ready<C, Verdict> {
        self.begin(now);
        let v = self.try_campaign();
        self.finish(v)
    }

    /// `suspect`: an edge in `from` reports that its receipt from member `about`, which it knew in `term`, missed
    /// CALL_DEADLINE, or that `about` answered `NotLeader` naming no leader. A report is the only hedge: it carries no
    /// command.
    ///
    /// While this member's promise runs, a report about the leader it promised, in this term, is counted per colo,
    /// and reports from `SUSPECT.0` colos within `SUSPECT.1` stop its promises to any leader for 2 x the window (a
    /// higher term restores them) and make its pre-vote refuse that leader for 2 x the window, across terms. Once the
    /// promise has lapsed, one report starts a campaign.
    pub fn suspect(&mut self, now: Timestamp, from: Colo, about: SiteId, term: Term) -> Ready<C, Verdict> {
        self.begin(now);
        let v = self.on_suspect(from, about, term);
        self.finish(v)
    }

    fn on_suspect(&mut self, from: Colo, about: SiteId, term: Term) -> Verdict {
        if self.role == Role::Leader {
            return Verdict::Leader;
        }
        if self.now < self.state.promise_until {
            // Only a report about the leader this member promised and has accepted appends from in this term counts:
            // a late report about a deposed leader, or a `NotLeader` from a candidate this member just voted for in
            // a transfer, withholds nothing from the new leader (found by the liveness runs, calls seed 1039).
            let leader = self.leader.filter(|l| *l != self.cfg.me && self.state.promised_to == Some(*l));
            if let Some(l) = leader.filter(|l| *l == about)
                && term == self.state.term
            {
                let (colos, window) = SUSPECT;
                let since = Timestamp(self.now.0.saturating_sub(window.as_millis() as u64));
                self.reports.retain(|_, t| *t >= since);
                self.reports.insert(from, self.now);
                if self.reports.len() >= colos {
                    let until = after(self.now, window * 2);
                    self.withhold_until = self.withhold_until.max(until);
                    let s = self.suspected.entry(l).or_insert(until);
                    *s = (*s).max(until);
                }
            }
            return Verdict::Promised;
        }
        self.try_campaign()
    }

    fn try_campaign(&mut self) -> Verdict {
        if self.role == Role::Leader {
            return Verdict::Leader;
        }
        if self.campaign.is_some() || self.boot_acks.is_some() {
            return Verdict::Busy;
        }
        // A learner never votes or leads, and never proposes the bootstrap entry: it joins a group that has one.
        if self.is_learner() {
            return Verdict::MayNotLead;
        }
        if self.fold.config().is_none() {
            return self.propose_bootstrap();
        }
        // A drained member, or one under `no_lead_until`, neither resumes nor campaigns, unless it has seen no leader
        // for 2 x ELECTION_AFTER, so a group whose every member is barred still elects.
        let waived = self.now >= after(self.last_leader_seen, ELECTION_AFTER * 2);
        let barred = self.no_lead() || self.drained.contains(&self.cfg.me);
        if barred && !waived {
            return Verdict::MayNotLead;
        }
        // A restarted leader resumes `led_term` after one round rather than start a new term.
        if self.state.led_term == Some(self.state.term) {
            self.become_leader(true);
            return Verdict::Resumed;
        }
        if self.now < self.state.promise_until {
            return Verdict::Promised;
        }
        if self.now < self.backoff_until || self.now < self.stand_aside_until {
            return Verdict::Busy;
        }
        self.start_campaign(true, None);
        Verdict::Started
    }

    /// Term 1 holds only the bootstrap entry, which any voter may propose because every voter builds the same bytes.
    fn propose_bootstrap(&mut self) -> Verdict {
        if self.last() == Index(0) {
            let e = self.boot_entry.clone();
            self.push_entry(e);
        }
        if !self.holds_boot() {
            // Index 1 holds some other entry. `verify` never stores one, so this is a corrupt store.
            return Verdict::Lost;
        }
        self.boot_acks = Some((BTreeSet::from([self.cfg.me]), self.now, self.now));
        self.send_proposals();
        Verdict::Bootstrapping
    }

    fn send_proposals(&mut self) {
        let peers: Vec<SiteId> = quorum::voters(self.boot_config()).into_iter().filter(|s| *s != self.cfg.me).collect();
        for p in peers {
            self.send(p, Message::Propose(self.boot_entry.clone()));
        }
    }

    pub(super) fn boot_config(&self) -> &GroupConfig {
        match &self.boot_entry.body {
            crate::EntryBody::Config(c) => c,
            _ => unreachable!(),
        }
    }

    pub(super) fn on_propose(&mut self, from: SiteId, e: crate::LogEntry<crate::Ev<C>>) {
        if self.last() == Index(0) && crate::digest(&e) == self.boot_digest {
            self.push_entry(e);
        }
        let stored = self.holds_boot();
        self.send(from, Message::Proposed { stored });
    }

    pub(super) fn on_proposed(&mut self, from: SiteId, stored: bool) {
        let Some((acks, _, _)) = self.boot_acks.as_mut() else { return };
        if !stored {
            return;
        }
        acks.insert(from);
        let acks = acks.clone();
        if quorum::commit(self.boot_config(), &acks) {
            self.boot_acks = None;
            self.set_commit(Index(1));
            // The member that took the creating call campaigns for term 2 once a quorum stores the entry.
            self.start_campaign(true, None);
        }
    }

    pub(super) fn start_campaign(&mut self, pre: bool, transfer: Option<TransferId>) {
        let term = Term(self.state.term.0 + 1);
        if !pre {
            self.state.term = term;
            self.state.voted_for = Some(self.cfg.me);
            self.state.led_term = None;
            self.state.follow_seq = 0;
            self.role = Role::Candidate;
            self.leader = None;
            self.dirty = true;
        }
        self.campaign = Some(Campaign {
            pre,
            term,
            granted: BTreeSet::from([self.cfg.me]),
            answered: BTreeSet::new(),
            started: self.now,
            sent_at: self.now,
            transfer,
        });
        self.send_votes();
        self.check_campaign();
    }

    fn send_votes(&mut self) {
        let Some(c) = &self.campaign else { return };
        let req = VoteReq {
            term: c.term,
            candidate: self.cfg.me,
            pre: c.pre,
            last_index: self.last(),
            last_term: self.last_term(),
            transfer: c.transfer,
            boots: self.state.boots,
            stepped_down_at: self.state.stepped_down_at,
            bootstrapped: self.state.commit >= Index(1),
        };
        let answered = c.answered.clone();
        let peers: Vec<SiteId> = quorum::voters(self.log_config())
            .into_iter()
            .filter(|s| *s != self.cfg.me && !answered.contains(s))
            .collect();
        for p in peers {
            self.send(p, Message::Vote(req.clone()));
        }
    }

    fn check_campaign(&mut self) {
        let Some(c) = &self.campaign else { return };
        if !quorum::election(self.log_config(), &c.granted) {
            return;
        }
        let (pre, transfer) = (c.pre, c.transfer);
        self.campaign = None;
        if pre {
            self.start_campaign(false, transfer);
        } else {
            self.out.resolutions.push(Resolution::Campaign(Verdict::Won));
            self.become_leader(false);
        }
    }

    fn lose_campaign(&mut self) {
        self.campaign = None;
        if self.role == Role::Candidate {
            self.role = Role::Follower;
        }
        // Two members campaigning at once would split votes forever; the loser waits a random 100 to 400 ms.
        let wait = 100 + self.rand() % 300;
        self.backoff_until = after(self.now, Duration::from_millis(wait));
        self.out.resolutions.push(Resolution::Campaign(Verdict::Lost));
    }

    pub(super) fn tick_election(&mut self) {
        if let Some((_, started, sent)) = self.boot_acks {
            if self.now >= after(started, CAMPAIGN_DEADLINE) {
                self.boot_acks = None;
                self.out.resolutions.push(Resolution::Campaign(Verdict::Lost));
            } else if self.now >= after(sent, CALL_DEADLINE) {
                self.boot_acks.as_mut().expect("checked").2 = self.now;
                self.send_proposals();
            }
        }
        let Some(c) = &self.campaign else { return };
        if self.now >= after(c.started, CAMPAIGN_DEADLINE) {
            self.lose_campaign();
        } else if self.now >= after(c.sent_at, CALL_DEADLINE) {
            if let Some(c) = self.campaign.as_mut() {
                c.sent_at = self.now;
            }
            self.send_votes();
        }
    }

    /// Moves to a higher term as a follower. Ends any leadership, which answers its tickets and reads `NotLeader`.
    pub(super) fn step_down(&mut self, term: Term) {
        if term > self.state.term {
            self.state.term = term;
            self.state.voted_for = None;
            self.state.led_term = None;
            self.state.follow_seq = 0;
            self.dirty = true;
            // A higher term restores promises that `suspect` stopped, and starts the colo count afresh. `suspected`
            // keeps refusing the reported leader in pre-vote.
            self.withhold_until = Timestamp(0);
            self.reports.clear();
            self.state.transfer_at = Timestamp(0);
            // The old term's leader leads no longer; this term's is known from its first append.
            if self.leader != Some(self.cfg.me) {
                self.leader = None;
            }
            if self.campaign.is_some() {
                self.lose_campaign();
            }
        }
        if self.role == Role::Candidate {
            self.role = Role::Follower;
        }
        if self.role == Role::Leader {
            self.end_leadership();
        }
    }

    pub(super) fn on_vote(&mut self, from: SiteId, req: VoteReq) {
        let res = self.vote(&req);
        if !req.pre && res.granted {
            self.dirty = true;
        }
        self.send(from, Message::VoteRes(res));
    }

    fn vote(&mut self, req: &VoteReq) -> VoteRes {
        let deny = |m: &Self, d: Deny| VoteRes {
            term: m.state.term,
            asked: req.term,
            pre: req.pre,
            granted: false,
            deny: Some(d),
        };
        // A candidate that holds the bootstrap entry as committed tells a voter that holds, or can build, the same
        // bytes that index 1 is committed.
        if self.fold.config().is_none() && req.bootstrapped {
            if self.last() == Index(0) {
                let e = self.boot_entry.clone();
                self.push_entry(e);
            }
            if self.holds_boot() {
                self.set_commit(Index(1));
            }
        }
        // A member with no committed config grants no vote.
        let Some((_, config)) = self.fold.config() else { return deny(self, Deny::NoConfig) };
        if !quorum::is_voter(config, req.candidate) {
            return deny(self, Deny::NotMember);
        }
        if req.term < self.state.term || (req.pre && req.term == self.state.term) {
            return deny(self, Deny::Stale);
        }
        // A voter disregards its promise only for a transfer vote naming the transfer id of the last AppendEntries it
        // accepted, or, as the old leader, for the target of its own transfer.
        let transfer_vote = req
            .transfer
            .is_some_and(|t| self.state.transfer == Some(t) || self.transferring() == Some((req.candidate, t)));
        if self.role == Role::Leader && !transfer_vote {
            return deny(self, Deny::Leader);
        }
        // A promise bars votes for a rival of the leader it was given to, not for that leader.
        if !transfer_vote && self.now < self.state.promise_until && self.state.promised_to != Some(req.candidate) {
            return deny(self, Deny::Promised);
        }
        if !req.pre && req.term > self.state.term {
            self.step_down(req.term);
        }
        let behind = (req.last_term, req.last_index) < (self.last_term(), self.last());
        if behind {
            return deny(self, Deny::LogBehind);
        }
        if req.pre {
            let newer = (req.last_term, req.last_index) > (self.last_term(), self.last());
            if self.campaign.is_some() && req.candidate > self.cfg.me && !newer {
                return deny(self, Deny::Contending);
            }
            let inside = |t: Timestamp| t.0 != 0 && after(t, RESET_QUARANTINE) > self.now;
            let flapping = inside(req.boots[0]) && inside(req.boots[1]);
            let stepped = req.stepped_down_at.is_some_and(inside);
            let waived = self.now >= after(self.last_leader_seen, ELECTION_AFTER * 2);
            // Two colos reported the candidate as this voter's leader: refused for 2 x SUSPECT's window, across
            // terms, so a gray leader cannot win back the next election.
            if self.suspected.get(&req.candidate).is_some_and(|u| self.now < *u) {
                return deny(self, Deny::Suspected);
            }
            if (flapping || stepped) && !waived {
                return deny(self, Deny::Quarantine);
            }
            if self.drained.contains(&req.candidate) && !waived {
                return deny(self, Deny::Drained);
            }
            return VoteRes { term: self.state.term, asked: req.term, pre: true, granted: true, deny: None };
        }
        if self.state.voted_for.is_some_and(|v| v != req.candidate) {
            return deny(self, Deny::Voted);
        }
        self.state.voted_for = Some(req.candidate);
        // A transfer vote is a promise to the target. Otherwise a follower's promise still names the old leader,
        // which it does not bar, and the old leader, which holds no promise, campaigns on the first `suspect` report
        // after the hand-off and deposes its own target (found by the liveness runs, seed 108).
        if transfer_vote {
            self.state.promise_until = self.state.promise_until.max(after(self.now, ELECTION_AFTER));
            self.state.promised_to = Some(req.candidate);
        }
        VoteRes { term: self.state.term, asked: req.term, pre: false, granted: true, deny: None }
    }

    pub(super) fn on_vote_res(&mut self, from: SiteId, res: VoteRes) {
        if res.term > self.state.term && !(res.pre && res.term < res.asked) {
            self.step_down(res.term);
            return;
        }
        let Some(c) = self.campaign.as_mut() else { return };
        if c.pre != res.pre || c.term != res.asked {
            return;
        }
        c.answered.insert(from);
        if res.pre && res.deny == Some(Deny::Suspected) {
            // Two colos reported this member as a gray leader: it stands aside instead of contending.
            self.stand_aside_until = after(self.now, SUSPECT.1 * 2);
        }
        let Some(c) = self.campaign.as_mut() else { return };
        if res.granted {
            c.granted.insert(from);
            self.check_campaign();
        } else if quorum::voters(self.log_config()).iter().all(|s| *s == self.cfg.me || c_answered(&self.campaign, *s))
        {
            self.lose_campaign();
        }
    }
}

fn c_answered(c: &Option<Campaign>, s: SiteId) -> bool {
    c.as_ref().is_some_and(|c| c.answered.contains(&s))
}
