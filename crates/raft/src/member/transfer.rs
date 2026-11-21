//! Leadership transfer (`design/DESIGN.md`, Shape, "Leadership", `transfer`).
//!
//! The leader stops appending and serving `fresh`, brings `to` to its last index, waits up to 2 x CALL_DEADLINE for
//! the other follower to accept an AppendEntries carrying the transfer id, and sends `TimeoutNow` until the LEASE
//! runs out. Every AppendEntries meanwhile carries `transfer: Some((to, id))`, and acks to those rounds extend no lease.
//! A transfer that has not completed within one LEASE is abandoned: the leader sends `transfer: None` and serves
//! `fresh` again only after one full round.

use graftyard_types::{SiteId, Timestamp};

use crate::fold::Command;
use crate::quorum;
use crate::types::{Term, TransferError, TransferId};
use crate::{CALL_DEADLINE, LEASE, RESET_QUARANTINE, after};

use super::{AbandonWhy, Alarm, Member, Message, Ready, Role, TransferWhy};

pub(super) struct Transfer {
    pub(super) id: TransferId,
    pub(super) to: SiteId,
    /// Started by `STEP_DOWN`: a leadership that ends while it runs sets `stepped_down_at`.
    pub(super) step_down: bool,
    /// A `/transfer` or drain hand-off: abandoned once its target's link would refuse it (F67 rule 13).
    guarded: bool,
    started: Timestamp,
    /// Rounds above this carry the transfer id.
    first_round: u64,
    /// When `to` first held the leader's last index.
    caught_up: Option<Timestamp>,
    /// The other follower accepted an AppendEntries carrying the id, so it will grant the target's transfer vote.
    pub(super) other_acked: bool,
    /// The target accepted one, so it takes the `TimeoutNow` for this id.
    to_acked: bool,
    timeout_sent: Option<Timestamp>,
}

impl<C: Command> Member<C> {
    /// Raft leadership transfer to `to`. Returns the transfer's id once it has started; the transfer completes when
    /// `to` wins `term + 1` (this member then follows), or is abandoned after one LEASE.
    pub fn transfer(&mut self, now: Timestamp, to: SiteId) -> Ready<C, Result<TransferId, TransferError>> {
        self.begin(now);
        let r = self.start_transfer(to, TransferWhy::Asked, true);
        self.finish(r)
    }

    /// The transfer this leader is running: its target and id.
    pub fn transferring(&self) -> Option<(SiteId, TransferId)> {
        self.lead.as_ref().and_then(|l| l.transfer.as_ref()).map(|t| (t.to, t.id))
    }

    /// May `s` take leadership from this leader: a voter of the latest config other than this member, not drained,
    /// not unable to follow, and not one this member refuses in pre-vote as `Suspected`. Such a target was reported
    /// as a gray leader by two colos while this member followed it; handed leadership, it led once more, half-hung,
    /// until check quorum moved it, and a drained leader handed off to it again: two hand-offs to the same sick site
    /// kept a group with two healthy voters leaderless for 37.4 s (F67 rule 10, live seed 12073 on main 0f7bf06).
    pub(super) fn may_lead_target(&self, s: SiteId) -> bool {
        s != self.cfg.me
            && quorum::is_voter(self.log_config(), s)
            && !self.drained.contains(&s)
            && !self.state.cannot_follow.contains(&s)
            && self.suspected.get(&s).is_none_or(|u| self.now >= *u)
    }

    /// `guarded`: a `/transfer` or drain hand-off, which F67 rule 13 holds to a target whose link from this leader does
    /// not lose answers while another's does not. `STEP_DOWN` picks its target by its own measure or by probes, and the
    /// move `home` by HOME_CALLS answered with none failed, so neither is guarded.
    /// `why` is logged (`Alarm::TransferStarted`); `TransferWhy::StepDown` makes it a `STEP_DOWN` transfer.
    pub(super) fn start_transfer(
        &mut self,
        to: SiteId,
        why: TransferWhy,
        guarded: bool,
    ) -> Result<TransferId, TransferError> {
        let step_down = why == TransferWhy::StepDown;
        let Some(lead) = &self.lead else { return Err(TransferError::NotLeader(self.not_leader())) };
        if !lead.ready() {
            return Err(TransferError::NotLeader(self.not_leader()));
        }
        if let Some(t) = &lead.transfer {
            return Err(TransferError::Busy(t.id));
        }
        if !self.may_lead_target(to) || self.quarantined(to) {
            return Err(TransferError::Target(to));
        }
        // On a half-hung target the hand-off won the transfer vote and led with its answers lost, or this leader served
        // nothing for the transfer's LEASE (F67 rule 13).
        if guarded && self.link_refused(to) {
            return Err(TransferError::Target(to));
        }
        let id = TransferId(self.rand());
        self.state.transfer = Some(id);
        self.dirty = true;
        let lead = self.lead.as_mut().expect("leading");
        lead.transfer = Some(Transfer {
            id,
            to,
            step_down,
            guarded,
            started: self.now,
            first_round: lead.round(),
            caught_up: None,
            other_acked: false,
            to_acked: false,
            timeout_sent: None,
        });
        // The followers disregard their promises for the target's transfer vote, so no lease holds from now.
        lead.reset_lease(self.now);
        self.drop_held_deps();
        self.out.alarms.push(Alarm::TransferStarted { to, why });
        self.broadcast_all();
        Ok(id)
    }

    /// An ok AppendEntries reply from `from` to round `round`.
    pub(super) fn transfer_ack(&mut self, from: SiteId, round: u64) {
        // A learner's ack says nothing about the transfer vote: only a voter grants one.
        let voter = quorum::is_voter(self.log_config(), from);
        let Some(t) = self.lead.as_mut().and_then(|l| l.transfer.as_mut()) else { return };
        if voter && round > t.first_round {
            if from == t.to {
                t.to_acked = true;
            } else {
                t.other_acked = true;
            }
        }
    }

    pub(super) fn tick_transfer(&mut self) {
        let Some(lead) = &self.lead else { return };
        let Some(t) = &lead.transfer else { return };
        let now = self.now;
        // A `/transfer` or drain hand-off stops once its target's link would refuse it, as it would not have started
        // (F67 rule 13): a target that went half-hung after it began held the group for the whole LEASE, and the
        // hand-off's reports barred this leader after it.
        if now >= after(t.started, LEASE) {
            self.abandon_transfer(AbandonWhy::Lease);
            return;
        }
        if t.guarded && self.link_refused(t.to) {
            self.abandon_transfer(AbandonWhy::LinkRefused);
            return;
        }
        let last = self.last();
        if lead.matched(t.to) < last || !t.to_acked {
            // `tick_leader` keeps sending the target its catch-up until the LEASE runs out.
            return;
        }
        let (to, id, other_acked, timeout_sent) = (t.to, t.id, t.other_acked, t.timeout_sent);
        let caught_up =
            *self.lead.as_mut().expect("leading").transfer.as_mut().expect("transfer").caught_up.get_or_insert(now);
        // Without this wait, a follower that had not seen the transfer refused the target's vote as promised, and the
        // group had no leader for 40 s in Milestone 0 (B0-07).
        let others = self.peers().into_iter().any(|p| p != to);
        if others && !other_acked && now < after(caught_up, CALL_DEADLINE * 2) {
            return;
        }
        if timeout_sent.is_none_or(|s| now >= after(s, CALL_DEADLINE)) {
            self.lead.as_mut().expect("leading").transfer.as_mut().expect("transfer").timeout_sent = Some(now);
            let term = self.state.term;
            self.send(to, Message::TimeoutNow { term, transfer: id });
        }
    }

    /// Ends the transfer with this member still leading: `transfer: None`, no lease, and `fresh` only after a round
    /// sent from now. The host logs `why` (`Alarm::TransferAbandoned`, F108).
    pub(super) fn abandon_transfer(&mut self, why: AbandonWhy) {
        let Some(lead) = self.lead.as_mut() else { return };
        let Some(t) = lead.transfer.take() else { return };
        lead.reset_lease(self.now);
        self.state.transfer = None;
        self.dirty = true;
        self.out.alarms.push(Alarm::TransferAbandoned { to: t.to, step_down: t.step_down, why });
        self.health_transfer_failed(t.to, t.step_down);
        self.broadcast_all();
        self.pump_after_transfer();
    }

    /// The target's side: campaign for `term + 1` at once with the transfer id, if this member may lead.
    pub(super) fn on_timeout_now(&mut self, from: SiteId, term: Term, transfer: TransferId) {
        // Only for the transfer of the last AppendEntries this member accepted: a `TimeoutNow` delayed past an
        // abandonment must not depose a leader whose new lease rests on this member's promise.
        if term != self.state.term
            || self.role != Role::Follower
            || self.leader != Some(from)
            || self.state.transfer != Some(transfer)
        {
            return;
        }
        if self.campaign.as_ref().is_some_and(|c| c.transfer() == Some(transfer)) {
            return;
        }
        if !self.may_take_lead() {
            self.send(from, Message::TransferRefused { term, transfer });
            return;
        }
        self.campaign = None;
        self.start_campaign(false, Some(transfer));
    }

    pub(super) fn on_transfer_refused(&mut self, term: Term, transfer: TransferId) {
        if term != self.state.term || self.transferring().is_none_or(|(_, id)| id != transfer) {
            return;
        }
        self.abandon_transfer(AbandonWhy::Refused);
    }

    /// Whether this member may campaign, resume `led_term`, or take a transfer: it has a committed config, its site is
    /// not drained, `no_lead_until` does not hold, and for a transfer, it is outside its own RESET_QUARANTINE.
    pub(super) fn may_take_lead(&self) -> bool {
        let inside = |t: Timestamp| t.0 != 0 && after(t, RESET_QUARANTINE) > self.now;
        let flapping = inside(self.state.boots[0]) && inside(self.state.boots[1]);
        let stepped = self.state.stepped_down_at.is_some_and(inside);
        self.fold.config().is_some()
            && !self.is_learner()
            && !self.no_lead()
            && !self.drained.contains(&self.cfg.me)
            && !flapping
            && !stepped
    }

    /// `no_lead_until` holds: an older follower refused this member's uncommitted entry under its current release.
    pub(super) fn no_lead(&self) -> bool {
        self.state.no_lead_until == Some(self.cfg.release.id)
    }
}
