//! `Health` and `STEP_DOWN` (`design/DESIGN.md`, Shape, "Chains", `Health`), the `linkErrors` probe, the drain
//! hand-off, and the transfer back to `home` (Shape, "Leadership").
//!
//! The member stays sans-IO: the host reports each member-to-member call that failed or missed CALL_DEADLINE
//! (`Member::call_failed`), and each storage confirm's begin and end (`Member::confirm_begin`, `Member::confirm_end`).
//! A reply that arrives is a call that succeeded.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

use graftyard_types::{SiteId, Timestamp};

use crate::fold::Command;
use crate::types::{GroupConfig, Term};
use crate::{CALL_DEADLINE, LOOP_LAG, RESET_QUARANTINE, STEP_DOWN, after};

use super::{
    Alarm, CallFailure, ConfirmId, ConfirmTimes, FailedCall, Member, Message, Ready, Role, StepDownWhy, StepDownWindow,
};

/// Pings in a `linkErrors` probe, five at a time: one failure in 20 is the 5% threshold.
const LINK_PINGS: u8 = 20;
const PING_BATCH: u8 = 5;
/// How long a leader waits for a probe's answer before the probe counts as unable to run.
const PROBE_WAIT: Duration = Duration::from_millis(4 * CALL_DEADLINE.as_millis() as u64 + 500);
/// Confirms are kept this long after they end, for the own-stall exclusion.
const CONFIRM_KEEP: Duration = Duration::from_secs(30);
/// The transfer back to `home` needs this many answered calls to `home`, and none failed, in one window [guess].
const HOME_CALLS: u32 = 5;
/// After a transfer to `home` is abandoned, the next try waits this long [guess].
const HOME_RETRY: Duration = Duration::from_secs(60);
/// The transfer back to `home` waits for a quiet leader (`Member::quiet`): at most this many commands queued [guess]
/// (`home-quiet`).
pub const HOME_QUIET_QUEUE: usize = 16;
/// How long the transfer back to `home` waits for a quiet leader after it is first due, before it goes anyway [guess]:
/// `home` is always regained.
pub const HOME_DEFER_MAX: Duration = Duration::from_secs(600);
/// step-busy: a loaded leader that committed at least this many entries in its `STEP_DOWN` window holds through its own
/// `CommitErrors` trip [guess: B0-33 m63's loaded leaders committed 7 to 19 entries in every 10 s, and a stuck one
/// commits none] (`decisions/DECISIONS.md`, `step-busy`).
pub const HOLD_ENTRIES: u64 = 5;

fn window() -> Duration {
    Duration::from_secs(u64::from(STEP_DOWN.window_s))
}

/// A leader's health window and its `STEP_DOWN` state. Cleared when leadership starts.
#[derive(Default)]
pub(super) struct LeaderHealth {
    /// Per commit-path call: when it ended, to whom, whether it succeeded, and whether the own-stall exclusion
    /// excused its failure.
    calls: VecDeque<(Timestamp, SiteId, bool, bool)>,
    /// Rounds already counted, so a duplicated reply or a failure report after a reply counts once.
    counted: BTreeSet<(SiteId, u64)>,
    /// Per storage confirm that ended in the window: when, and what was measured of it.
    confirms: VecDeque<Ended>,
    /// Followers that answered an AppendEntries in this leadership. A failed call to one not yet heard from is not
    /// counted: the quorum commits without a follower it has not reached.
    heard: BTreeSet<SiteId>,
    /// Per follower, the send time of the last deadline miss counted.
    missed: BTreeMap<SiteId, Timestamp>,
    /// Per follower, when each commit-path call to it ended in the window, and whether it was answered: every one,
    /// unlike `calls`, which counts no failure before the follower first answered and one miss per stall.
    links: VecDeque<(Timestamp, SiteId, bool)>,
    /// Per answered AppendEntries in the window: when it was answered, by whom, and its round trip in ms. A follower
    /// answers after its own storage confirms the entries, so its slow confirms show here.
    rtts: VecDeque<(Timestamp, SiteId, u32)>,
    /// Per commit this leadership counted in the window: when, and how many entries it committed (`busy`).
    commits: VecDeque<(Timestamp, u64)>,
    hold_until: Option<Timestamp>,
    probe: Option<LeaderProbe>,
    /// From each follower's AppendEntries replies: its `stepped_down_at` and `boots`.
    pub(super) peer_quarantine: BTreeMap<SiteId, (Option<Timestamp>, [Timestamp; 2])>,
    /// No transfer back to `home` before this: one to it was abandoned.
    home_after: Option<Timestamp>,
    /// When `home` first counted as recovered in this leadership, and has since: HOME_DEFER_MAX runs from it.
    home_due: Option<Timestamp>,
    /// No automatic hand-off before this: the last one was abandoned.
    handoff_after: Option<Timestamp>,
}

/// One storage confirm that ended in a leader's window.
struct Ended {
    at: Timestamp,
    id: ConfirmId,
    /// From `confirm_begin` to its end, on the member clock.
    whole: u32,
    /// Its storage (`ConfirmTimes::storage_ms`; `whole` when the host did not say).
    storage: u32,
    /// From its step to its end on the isolate's clock (`ConfirmTimes::isolate_ms`; `whole` when the host did not say).
    isolate: u32,
    sync: Option<u64>,
}

impl Ended {
    /// The sample `LOOP_LAG` counts it in: its sync, or itself when the host named none.
    fn sample(&self) -> (bool, u64) {
        self.sync.map_or((false, self.id.0), |s| (true, s))
    }
}

struct LeaderProbe {
    id: u64,
    why: StepDownWhy,
    started: Timestamp,
    sent_at: Timestamp,
    /// Per candidate: the probe's failed share once it answered.
    results: BTreeMap<SiteId, Option<f32>>,
}

/// A `linkErrors` probe a follower runs for its leader: LINK_PINGS pings to `target`, PING_BATCH at a time.
pub(super) struct Prober {
    leader: SiteId,
    term: Term,
    id: u64,
    target: SiteId,
    /// Per ping in the current batch: sent at, and whether it answered (`Some(true)`) or failed (`Some(false)`).
    batch: BTreeMap<u8, (Timestamp, Option<bool>)>,
    sent: u8,
    failed: u8,
}

impl<C: Command> Member<C> {
    // ---- inputs ----

    /// The host saw a call this member sent fail: an error, or no reply within CALL_DEADLINE. A reply that still
    /// arrives later may be passed to `step`; the call counts once, as failed.
    pub fn call_failed(&mut self, now: Timestamp, f: CallFailure) -> Ready<C, ()> {
        self.begin(now);
        match f.call {
            FailedCall::Append { term, boot, round, sent_at } => {
                // An error ends the call; a missed deadline does not, because the host still awaits it.
                if boot == self.boot && !f.deadline {
                    self.calls_out.remove(&(f.to, round));
                }
                if self.role == Role::Leader && term == self.state.term && boot == self.boot {
                    self.record_call(f.to, round, false, f.deadline, sent_at);
                    self.append_failed(f.to, round);
                    self.check_health();
                }
            }
            FailedCall::Ping { probe, n } => self.on_pong(f.to, probe, n, false),
        }
        self.finish(())
    }

    /// The host ended a call this member sent: answered, failed, or given up LATE_LIMIT past its deadline. Its
    /// AppendEntries bytes stop counting against PEER_OUT_MAX, LEARNERS_OUT_MAX, and OUT_MAX (`leader::out_allows`).
    /// A host calls it only for a call of this boot.
    pub fn call_ended(&mut self, now: Timestamp, to: SiteId, round: u64) -> Ready<C, ()> {
        self.begin(now);
        self.calls_out.remove(&(to, round));
        self.finish(())
    }

    /// AppendEntries bytes alive on this member's calls (`call_ended`), for the host's memory split.
    pub fn calls_out_bytes(&self) -> usize {
        self.calls_out.values().map(|(_, b)| b).sum()
    }

    /// A storage confirm began: the host started making this member's writes durable (every `Ready` with writes,
    /// before its outputs leave).
    pub fn confirm_begin(&mut self, now: Timestamp) -> ConfirmId {
        self.next_confirm += 1;
        let id = ConfirmId(self.next_confirm);
        self.confirms.insert(id, (now, None));
        id
    }

    /// The confirm `id` ended: its writes are durable. Its storage took the whole confirm: a host whose transaction
    /// runs at `confirm_begin`.
    pub fn confirm_end(&mut self, now: Timestamp, id: ConfirmId) -> Ready<C, ()> {
        self.confirm_ended(now, id, None)
    }

    /// The confirm `id` ended, and the host measured it (`ConfirmTimes`). `STEP_DOWN`'s confirm rule judges its storage
    /// (`decisions/DECISIONS.md`, "`confirm-storage`"), and `LOOP_LAG` its time on the isolate's clock, one sample per
    /// sync (`LOOP_LAG`'s 2026-11-20 amendment). The own-stall exclusion and the trip's `kept` samples read the whole
    /// confirm on the member clock, because the output gate holds every send for all of it.
    pub fn confirm_end_timed(&mut self, now: Timestamp, id: ConfirmId, times: ConfirmTimes) -> Ready<C, ()> {
        self.confirm_ended(now, id, Some(times))
    }

    fn confirm_ended(&mut self, now: Timestamp, id: ConfirmId, times: Option<ConfirmTimes>) -> Ready<C, ()> {
        self.begin(now);
        if let Some((began, end)) = self.confirms.get_mut(&id) {
            *end = Some(now);
            let ms = now.0.saturating_sub(began.0) as u32;
            let began = *began;
            // Two clocks: storage measured on the host's cannot be longer than the confirm it is part of.
            let storage = times.map_or(ms, |t| t.storage_ms.min(ms));
            let isolate = times.and_then(|t| t.isolate_ms).unwrap_or(ms);
            let sync = times.and_then(|t| t.sync);
            if let Some(lead) = self.lead.as_mut()
                && began >= lead.started()
            {
                lead.health.confirms.push_back(Ended { at: now, id, whole: ms, storage, isolate, sync });
                self.check_health();
            }
        }
        let keep = Timestamp(now.0.saturating_sub(CONFIRM_KEEP.as_millis() as u64));
        self.confirms.retain(|_, (_, e)| e.is_none_or(|e| e >= keep));
        self.finish(())
    }

    /// The other sites' `SiteHealth` verdict on this member's site (`design/DESIGN.md`, Shape, `SiteHealth`): both other
    /// sites count it failing their calls and receipts. A leader reads it only to let a hot group's own `STEP_DOWN`
    /// trip move it (`step-agree`); the host sets it when the verdict changes.
    pub fn set_site_sick(&mut self, now: Timestamp, sick: bool) -> Ready<C, ()> {
        self.begin(now);
        self.site_sick = sick;
        self.tick_health();
        self.finish(())
    }

    /// The sites whose members may not lead (drained, or under the first day's lead limit), from the `Release`
    /// group's site modes. A leader on such a site hands off within one LEASE.
    pub fn set_drained(&mut self, now: Timestamp, sites: impl IntoIterator<Item = SiteId>) -> Ready<C, ()> {
        self.begin(now);
        self.drained = sites.into_iter().collect();
        self.tick_health();
        self.finish(())
    }

    /// The `Release` group recorded a rollback of the site whose refusal set `no_lead_until`.
    pub fn clear_no_lead(&mut self, now: Timestamp) -> Ready<C, ()> {
        self.begin(now);
        if self.state.no_lead_until.take().is_some() {
            self.dirty = true;
        }
        self.finish(())
    }

    // ---- the leader's window ----

    /// One commit-path call by the leader ended: a reply to round `round` (`ok`), or a failure the host reported.
    pub(super) fn record_call(&mut self, to: SiteId, round: u64, ok: bool, deadline: bool, sent_at: Timestamp) {
        let mut excused = !ok && deadline && self.own_stall(sent_at);
        let now = self.now;
        let Some(lead) = self.lead.as_mut() else { return };
        let h = &mut lead.health;
        if ok {
            h.heard.insert(to);
            h.rtts.push_back((now, to, now.0.saturating_sub(sent_at.0) as u32));
        }
        if !h.counted.contains(&(to, round)) {
            h.links.push_back((now, to, ok));
        }
        if !h.counted.insert((to, round)) || (!ok && !h.heard.contains(&to)) {
            return;
        }
        // One stall of a follower counts once per CALL_DEADLINE: a miss sent within CALL_DEADLINE after the last counted
        // miss to it is the same stall. A lapsed lease starts a round per read, so a 1.9 s stall missed 17 to 31 calls at
        // once and moved healthy identity leaders (LM-1, round 8).
        if !ok && deadline && !excused {
            if h.missed.get(&to).is_some_and(|l| sent_at >= *l && sent_at < after(*l, CALL_DEADLINE)) {
                excused = true;
            } else {
                h.missed.insert(to, sent_at);
            }
        }
        h.calls.push_back((now, to, ok, excused));
    }

    /// A confirm of this member's own storage began at or before `sent_at` and was unconfirmed at the call's deadline.
    fn own_stall(&self, sent_at: Timestamp) -> bool {
        let deadline = after(sent_at, CALL_DEADLINE);
        self.confirms.values().any(|(b, e)| *b <= sent_at && e.is_none_or(|e| e > deadline))
    }

    fn prune(&mut self) {
        let since = Timestamp(self.now.0.saturating_sub(window().as_millis() as u64));
        let Some(lead) = self.lead.as_mut() else { return };
        let keep = lead.round().saturating_sub(2048);
        let h = &mut lead.health;
        while h.calls.front().is_some_and(|c| c.0 < since) {
            h.calls.pop_front();
        }
        while h.links.front().is_some_and(|c| c.0 < since) {
            h.links.pop_front();
        }
        while h.rtts.front().is_some_and(|c| c.0 < since) {
            h.rtts.pop_front();
        }
        while h.commits.front().is_some_and(|c| c.0 < since) {
            h.commits.pop_front();
        }
        while h.confirms.front().is_some_and(|c| c.at < since) {
            h.confirms.pop_front();
        }
        if h.counted.len() > 4096 {
            h.counted.retain(|(_, r)| *r >= keep);
        }
    }

    /// Per follower: counted calls and failed calls in the window, after the own-stall exclusion.
    fn call_counts(&self) -> Vec<(SiteId, u32, u32)> {
        let Some(lead) = &self.lead else { return Vec::new() };
        self.peers()
            .into_iter()
            .map(|p| {
                let mine = lead.health.calls.iter().filter(|c| c.1 == p && !c.3);
                let (n, failed) = mine.fold((0, 0), |(n, f), c| (n + 1, f + u32::from(!c.2)));
                (p, n, failed)
            })
            .collect()
    }

    fn rate(n: u32, failed: u32) -> f32 {
        if n == 0 { 0.0 } else { failed as f32 / n as f32 }
    }

    /// This leader's calls to `s` fail in the window over `commit_errors`, with at least `commit_fail_min` failed, as
    /// the commit-error rule counts them: one or two misses in a quiet window pass 5% on noise alone (F37).
    pub(super) fn link_failing(&self, s: SiteId) -> bool {
        self.call_counts().iter().find(|c| c.0 == s).is_some_and(|c| {
            Self::rate(c.1, c.2) > STEP_DOWN.commit_errors && c.2 >= u32::from(STEP_DOWN.commit_fail_min)
        })
    }

    /// This leader's calls to `s` in the window fail as the commit-error rule counts a failing link (over
    /// `commit_errors`, at least `commit_fail_min`), but over every call: `call_counts` leaves out the misses before `s`
    /// first answered and the repeats within one stall, which on a half-hung follower are most of them (F67 rule 13).
    pub(super) fn link_lossy(&self, s: SiteId) -> bool {
        let Some(lead) = &self.lead else { return false };
        let mine = lead.health.links.iter().filter(|c| c.1 == s);
        let (n, failed) = mine.fold((0u32, 0u32), |(n, f), c| (n + 1, f + u32::from(!c.2)));
        failed >= u32::from(STEP_DOWN.commit_fail_min) && Self::rate(n, failed) > STEP_DOWN.commit_errors
    }

    /// `s` is no target for a `/transfer` or drain hand-off: its link is lossy while another follower's is not. When
    /// every link is lossy the fault is likely this leader's own, which handing off cures, so any target qualifies
    /// (F67 rule 13).
    pub(super) fn link_refused(&self, s: SiteId) -> bool {
        self.link_lossy(s) && self.peers().into_iter().any(|p| p != s && !self.link_lossy(p))
    }

    /// This member's own storage would trip `STEP_DOWN`'s confirm rule once it led: over the window, at least
    /// `commit_fail_min` confirms took over `confirm_p99_ms`, ended or still running, and so does the p99. A follower
    /// confirms about once per append it accepts, fewer than a leader, so it needs fewer than the leader's
    /// `confirm_slow_min`: at six, one with 4 of 11 confirms at 1.05 s read healthy, campaigned against the leader a
    /// LoopLag hand-off had just made, deposed it on the higher term and was refused LogBehind, and nobody led for
    /// 12.5 s (`tests/m0/results/looplag-handoff.md`, seed 910).
    pub(super) fn storage_slow(&self) -> bool {
        let since = Timestamp(self.now.0.saturating_sub(window().as_millis() as u64));
        let mut ms: Vec<u64> = self
            .confirms
            .values()
            .filter(|(_, e)| e.is_none_or(|e| e >= since))
            .map(|(b, e)| e.unwrap_or(self.now).0.saturating_sub(b.0))
            .collect();
        ms.sort_unstable();
        let p99 = ms.get(ms.len() * 99 / 100).copied().unwrap_or(0);
        let slow = ms.iter().filter(|m| **m > u64::from(STEP_DOWN.confirm_p99_ms)).count();
        p99 > u64::from(STEP_DOWN.confirm_p99_ms) && slow >= usize::from(STEP_DOWN.commit_fail_min)
    }

    /// `s`'s storage looks as slow as STEP_DOWN's confirm rule judges a leader's own: at least `confirm_slow_min` of
    /// its answers in the window took over `confirm_p99_ms` longer than its fastest. A leader whose own confirms
    /// tripped hands off to no such follower, which would trip the same way (LM-1's k-4 churn).
    pub(super) fn peer_storage_slow(&self, s: SiteId) -> bool {
        let Some(lead) = &self.lead else { return false };
        let mine = || lead.health.rtts.iter().filter(|c| c.1 == s).map(|c| c.2);
        let Some(fastest) = mine().min() else { return false };
        let slow = mine().filter(|ms| *ms > fastest + STEP_DOWN.confirm_p99_ms).count();
        slow >= usize::from(STEP_DOWN.commit_fail_min)
    }

    /// The commit-advance path committed `entries` more entries in this leadership.
    pub(super) fn note_commit(&mut self, entries: u64) {
        let now = self.now;
        if let Some(lead) = self.lead.as_mut() {
            lead.health.commits.push_back((now, entries));
        }
    }

    /// step-busy's loaded leader: it committed at least `HOLD_ENTRIES` entries in the `STEP_DOWN` window and is not
    /// `quiet` (more than HOME_QUIET_QUEUE commands queued, or a large entry in flight), so a transfer would move a
    /// backlog that keeps committing. A quiet leader on a gray link still commits too, and moves as before.
    /// Read after `prune`, which keeps only the window's commits.
    fn busy(&self) -> bool {
        let Some(lead) = &self.lead else { return false };
        let committed: u64 = lead.health.commits.iter().map(|c| c.1).sum();
        committed >= HOLD_ENTRIES && !self.quiet(HOME_QUIET_QUEUE)
    }

    pub(super) fn quarantined(&self, s: SiteId) -> bool {
        let inside = |t: Timestamp| t.0 != 0 && after(t, RESET_QUARANTINE) > self.now;
        let Some((stepped, boots)) = self.lead.as_ref().and_then(|l| l.health.peer_quarantine.get(&s)) else {
            return false;
        };
        stepped.is_some_and(inside) || (inside(boots[0]) && inside(boots[1]))
    }

    /// What tripped `STEP_DOWN` in the window, if anything.
    fn tripped(&self) -> Option<StepDownWhy> {
        let lead = self.lead.as_ref()?;
        let counts = self.call_counts();
        let errors = !counts.is_empty()
            && counts.iter().all(|(_, n, f)| {
                Self::rate(*n, *f) > STEP_DOWN.commit_errors && *f >= u32::from(STEP_DOWN.commit_fail_min)
            });
        if errors {
            return Some(StepDownWhy::CommitErrors { calls: counts });
        }
        let mut ms: Vec<u32> = lead.health.confirms.iter().map(|c| c.storage).collect();
        ms.sort_unstable();
        let p99 = ms.get(ms.len() * 99 / 100).copied().unwrap_or(0);
        let slow = ms.iter().filter(|m| **m > STEP_DOWN.confirm_p99_ms).count() as u32;
        if p99 > STEP_DOWN.confirm_p99_ms && slow >= u32::from(STEP_DOWN.confirm_slow_min) {
            return Some(StepDownWhy::Confirm { p99_ms: p99, slow, of: ms.len() as u32 });
        }
        // A lagging loop: confirms that read whole ticks on the isolate's clock, where workerd's clamp is. Its confirms
        // stay under `confirm_p99_ms` often enough that the confirm rule never tripped in B0-02's runs with the
        // signature (`tests/m0/results/carried-8.md`). The confirms one sync ends read one span, so they are one sample.
        let (min, within, at_least) = LOOP_LAG;
        let since = Timestamp(self.now.0.saturating_sub(within.as_millis() as u64));
        let clamped = lead
            .health
            .confirms
            .iter()
            .filter(|c| c.at >= since && c.isolate >= at_least && c.isolate % 100 == 0)
            .map(Ended::sample)
            .collect::<BTreeSet<_>>()
            .len() as u32;
        if clamped >= min {
            return Some(StepDownWhy::LoopLag { clamped });
        }
        None
    }

    /// `STEP_DOWN`: a leader sick by its own measure transfers to a qualifying follower, or holds for one window.
    pub(super) fn check_health(&mut self) {
        self.prune();
        let Some(lead) = &self.lead else { return };
        let held = lead.health.hold_until.is_some_and(|u| self.now < u);
        if !lead.ready() || lead.transfer.is_some() || lead.health.probe.is_some() || held {
            return;
        }
        let Some(why) = self.tripped() else { return };
        // step-agree: a hot group's own tail moves its leader only when the other sites agree its site is sick or the
        // group stopped committing. `hot` counts committed pushes over HOT_WINDOW, so a group that stops committing
        // stops being hot within it, and its next trip after this hold moves it. A loaded leader that still
        // commits holds: B0-35 m60's term 3 was such a leader, and the move cost the load a term change
        // (`decisions/DECISIONS.md`, `step-agree`). step-busy: a `CommitErrors` trip also holds, hot or not, on a loaded
        // leader that committed entries in the window. B0-33 m63's leader committed 13 entries in the window of the
        // trip that moved it, with agent commands queued, while `hot` read its bursts of pushes as cold
        // (`decisions/DECISIONS.md`, `step-busy`).
        let errors_trip = matches!(why, StepDownWhy::CommitErrors { .. });
        if (self.hot() || (errors_trip && self.busy())) && !self.site_sick {
            self.hold(why);
            return;
        }
        let failing = |s: SiteId| self.link_failing(s);
        // A follower that may lead, outside RESET_QUARANTINE; on a confirm or loop-lag trip also one whose link from
        // this leader is not failing and whose own storage does not look as slow.
        let candidates: Vec<SiteId> = self
            .peers()
            .into_iter()
            .filter(|s| self.may_lead_target(*s) && !self.quarantined(*s) && !self.peer_storage_slow(*s))
            .filter(|s| errors_trip || !failing(*s))
            .collect();
        if candidates.is_empty() {
            self.hold(why);
            return;
        }
        if !errors_trip {
            self.step_down_to(why, candidates);
            return;
        }
        // On a commit-error trip the leader's calls fail to every follower by definition, so a target qualifies on the
        // other follower's calls to it: that follower probes it.
        let id = self.rand();
        let term = self.state.term;
        let peers = self.peers();
        let mut results = BTreeMap::new();
        for t in candidates {
            results.insert(t, None);
            if let Some(other) = peers.iter().copied().find(|p| *p != t) {
                self.send(other, Message::Probe { term, id, target: t });
            }
        }
        let now = self.now;
        self.lead.as_mut().expect("leading").health.probe =
            Some(LeaderProbe { id, why, started: now, sent_at: now, results });
    }

    /// The window `tripped` judged, kept for the host's log (`Member::step_down_window`); no decision reads it.
    fn note_trip(&mut self) {
        let Some(lead) = self.lead.as_ref() else { return };
        let confirms = lead.health.confirms.iter().map(|c| (c.at, c.storage)).collect();
        let whole = lead.health.confirms.iter().map(|c| (c.at, c.whole)).collect();
        let isolate = lead.health.confirms.iter().map(|c| (c.at, c.isolate, c.sync)).collect();
        let started = lead.started();
        let mut kept: Vec<(Timestamp, u32)> = self
            .confirms
            .values()
            .filter(|(b, _)| *b >= started)
            .filter_map(|(b, e)| e.map(|e| (e, e.0.saturating_sub(b.0) as u32)))
            .collect();
        kept.sort_unstable();
        let since = Timestamp(self.now.0.saturating_sub(window().as_millis() as u64));
        self.last_trip = Some(StepDownWindow { from: since, to: self.now, confirms, whole, isolate, kept });
    }

    fn hold(&mut self, why: StepDownWhy) {
        self.note_trip();
        let until = after(self.now, window());
        if let Some(lead) = self.lead.as_mut() {
            lead.health.hold_until = Some(until);
        }
        self.out.alarms.push(Alarm::StepDown { why, to: None });
    }

    /// Transfers to the qualifying candidate whose calls failed least, then the most caught up.
    fn step_down_to(&mut self, why: StepDownWhy, mut ok: Vec<SiteId>) {
        let counts = self.call_counts();
        let failed = |s: SiteId| counts.iter().find(|c| c.0 == s).map_or(0, |c| c.2);
        let lead = self.lead.as_ref().expect("leading");
        ok.sort_by_key(|s| (failed(*s), std::cmp::Reverse(lead.matched(*s))));
        let Some(to) = ok.first().copied() else {
            self.hold(why);
            return;
        };
        match self.start_transfer(to, true, false) {
            Ok(_) => {
                self.note_trip();
                self.out.alarms.push(Alarm::StepDown { why, to: Some(to) })
            }
            Err(_) => self.hold(why),
        }
    }

    pub(super) fn on_probe_res(&mut self, term: Term, id: u64, target: SiteId, failed: u8, of: u8) {
        if term != self.state.term {
            return;
        }
        let Some(p) = self.lead.as_mut().and_then(|l| l.health.probe.as_mut()) else { return };
        if p.id != id {
            return;
        }
        if let Some(r) = p.results.get_mut(&target) {
            *r = Some(if of == 0 { 1.0 } else { f32::from(failed) / f32::from(of) });
        }
        if p.results.values().all(Option::is_some) {
            self.finish_probe();
        }
    }

    /// Decides on the probes' answers. A probe that did not answer disqualifies its target.
    fn finish_probe(&mut self) {
        let Some(p) = self.lead.as_mut().and_then(|l| l.health.probe.take()) else { return };
        let ok: Vec<SiteId> = p
            .results
            .iter()
            .filter(|(_, r)| r.is_some_and(|f| f <= STEP_DOWN.commit_errors))
            .map(|(s, _)| *s)
            .collect();
        let still: Vec<SiteId> = ok.into_iter().filter(|s| self.may_lead_target(*s) && !self.quarantined(*s)).collect();
        self.step_down_to(p.why, still);
    }

    pub(super) fn tick_health(&mut self) {
        let Some(lead) = &self.lead else { return };
        if lead.health.probe.as_ref().is_some_and(|p| self.now >= after(p.started, PROBE_WAIT)) {
            self.finish_probe();
        } else if let Some(p) = &lead.health.probe
            && self.now >= after(p.sent_at, CALL_DEADLINE)
        {
            // A probe request or its answer lost on the way is asked again, like any member call.
            let (id, term, peers) = (p.id, self.state.term, self.peers());
            let waiting: Vec<SiteId> = p.results.iter().filter(|(_, r)| r.is_none()).map(|(s, _)| *s).collect();
            self.lead.as_mut().expect("leading").health.probe.as_mut().expect("probe").sent_at = self.now;
            for t in waiting {
                if let Some(other) = peers.iter().copied().find(|p| *p != t) {
                    self.send(other, Message::Probe { term, id, target: t });
                }
            }
        }
        self.classify_refusals();
        self.check_health();
        self.maybe_hand_off();
    }

    /// A leader on a drained site, or one whose group's `home` has recovered and caught up, transfers.
    fn maybe_hand_off(&mut self) {
        let Some(lead) = &self.lead else { return };
        if !lead.ready() || lead.transfer.is_some() || lead.health.probe.is_some() {
            return;
        }
        if lead.health.handoff_after.is_some_and(|t| self.now < t) {
            return;
        }
        if self.drained.contains(&self.cfg.me) || self.no_lead() {
            self.hand_off();
            return;
        }
        let home = self.home();
        if home == self.cfg.me {
            return;
        }
        // Recovered: `home` may lead, is outside RESET_QUARANTINE, and answered at least HOME_CALLS calls in the window
        // with none failed. Only a `home` that stops being recovered restarts HOME_DEFER_MAX.
        let calls = lead.health.calls.iter().filter(|c| c.1 == home);
        let (n, failed) = calls.fold((0u32, 0u32), |(n, f), c| (n + 1, f + u32::from(!c.2)));
        if !self.may_lead_target(home) || self.quarantined(home) || n < HOME_CALLS || failed > 0 {
            self.lead.as_mut().expect("leading").health.home_due = None;
            return;
        }
        // Caught up: `home` holds every committed entry. The transfer itself brings it to the last index; under load
        // one entry is always in flight, so asking for the last index first would rarely hold.
        if lead.matched(home) < self.state.commit
            || self.now < after(lead.started(), window())
            || lead.health.home_after.is_some_and(|t| self.now < t)
            || self.tripped().is_some()
        {
            return;
        }
        // home-quiet: due, the transfer waits for a quiet leader, because it stalls whatever waits on this one until
        // `home` serves (B0-35 m58's term 4: dispatch p99 15.1 s that minute against 2.7 s before), and goes anyway
        // HOME_DEFER_MAX after it was first due.
        let now = self.now;
        let due = *self.lead.as_mut().expect("leading").health.home_due.get_or_insert(now);
        if self.quiet(HOME_QUIET_QUEUE) || now >= after(due, HOME_DEFER_MAX) {
            let _ = self.start_transfer(home, false, false);
        }
    }

    /// The group's preferred leader site: the `home` of the latest config's placement, which a config change may move
    /// (an owner move re-homes the group), not the bootstrap record's.
    fn home(&self) -> SiteId {
        match self.log_config() {
            GroupConfig::Normal(p) | GroupConfig::Degraded { placement: p, .. } => p.home(),
            GroupConfig::Joint { new, .. } => new.home(),
        }
    }

    /// A transfer was abandoned. After a `STEP_DOWN` transfer the leader holds for one window before it judges
    /// itself again, so it does not retry at once onto the same target.
    pub(super) fn health_transfer_failed(&mut self, to: SiteId, step_down: bool) {
        let (now, home) = (self.now, self.home());
        let Some(lead) = self.lead.as_mut() else { return };
        if step_down {
            lead.health.hold_until = Some(after(now, window()));
        } else if to == home {
            lead.health.home_after = Some(after(now, HOME_RETRY));
        }
        lead.health.handoff_after = Some(after(now, CALL_DEADLINE * 2));
    }

    // ---- the follower's probe ----

    pub(super) fn on_probe(&mut self, from: SiteId, term: Term, id: u64, target: SiteId) {
        if term != self.state.term || self.leader != Some(from) || self.role != Role::Follower || target == self.cfg.me
        {
            return;
        }
        if let Some((done_id, done_target, failed, of)) = self.probed
            && done_id == id
            && done_target == target
        {
            self.send(from, Message::ProbeRes { term, id, target, failed, of });
            return;
        }
        if self.probing.as_ref().is_some_and(|p| p.id == id && p.target == target) {
            return;
        }
        self.probing = Some(Prober { leader: from, term, id, target, batch: BTreeMap::new(), sent: 0, failed: 0 });
        self.send_pings();
    }

    fn send_pings(&mut self) {
        let now = self.now;
        let Some(p) = self.probing.as_mut() else { return };
        let (probe, target, start) = (p.id, p.target, p.sent);
        let n = PING_BATCH.min(LINK_PINGS - p.sent);
        p.batch = (start..start + n).map(|i| (i, (now, None))).collect();
        p.sent += n;
        for i in start..start + n {
            self.send(target, Message::Ping { probe, n: i });
        }
    }

    pub(super) fn on_pong(&mut self, from: SiteId, probe: u64, n: u8, ok: bool) {
        let Some(p) = self.probing.as_mut() else { return };
        if p.id != probe || p.target != from {
            return;
        }
        if let Some((_, r)) = p.batch.get_mut(&n)
            && r.is_none()
        {
            *r = Some(ok);
        }
        self.advance_probe();
    }

    pub(super) fn tick_probe(&mut self) {
        let now = self.now;
        let Some(p) = self.probing.as_mut() else { return };
        for (sent, r) in p.batch.values_mut() {
            if r.is_none() && now >= after(*sent, CALL_DEADLINE) {
                *r = Some(false);
            }
        }
        self.advance_probe();
    }

    fn advance_probe(&mut self) {
        let Some(p) = self.probing.as_mut() else { return };
        if p.batch.values().any(|(_, r)| r.is_none()) {
            return;
        }
        p.failed += p.batch.values().filter(|(_, r)| *r == Some(false)).count() as u8;
        p.batch.clear();
        if p.sent < LINK_PINGS {
            self.send_pings();
            return;
        }
        let p = self.probing.take().expect("probing");
        self.probed = Some((p.id, p.target, p.failed, p.sent));
        let msg = Message::ProbeRes { term: p.term, id: p.id, target: p.target, failed: p.failed, of: p.sent };
        self.send(p.leader, msg);
    }
}
