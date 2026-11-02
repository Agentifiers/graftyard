# M1-WP3: Leadership and health (pure crate)

Run 2026-11-01 on `crates/raft` (`graftyard-raft`), the sans-IO member from M1-WP2. Built here: `transfer`,
`suspect` with `SUSPECT`, `Health` and `STEP_DOWN`, the `linkErrors` probe, the drain hand-off, the transfer back to
`home`, the release-aware handling of `verify` refusals (`no_lead_until`, `cannot_follow`), and an edge `Receipt` type
that makes BF-14 structural. The crate is still not hosted in a Durable Object, so every number below is from the
simulator, not the real stack. BF-14 and BM-1's "one site's member class failing mid-run" still need the hosted code.

## What the host now reports

The member stays sans-IO. Three inputs were added for `Health`, plus two for site state:

- `Member::call_failed(now, CallFailure)`: a member-to-member call this member sent failed, or no reply came within
  CALL_DEADLINE. `Envelope::failure(deadline)` builds the report from the request the member released, so the host
  keeps the envelope until the reply or the deadline. Only AppendEntries and probe pings are counted. `deadline`
  says the call missed CALL_DEADLINE rather than failing with an error, since only a miss can be excused by the
  member's own stall. A reply that arrives after its failure was reported may still be passed to `step`; each round
  counts once.
- `Member::confirm_begin(now) -> ConfirmId` and `Member::confirm_end(now, id)`: one storage confirm, begun when the
  host starts making a `Ready`'s writes durable and ended when they are. The leader's confirms feed the confirm rule,
  and their spans feed the own-stall exclusion.
- `Member::set_drained(now, sites)`: the sites that may not lead (drained, or under the first day's lead limit), from
  the `Release` group's site modes.
- `Member::clear_no_lead(now)`: the `Release` group recorded a rollback of the site whose refusal set
  `no_lead_until`. A change of this member's own release clears it at `restore`.

A successful call is any AppendEntries reply that reaches `step`.

## Public API changes

No M1-WP2 signature changed except `AppendReq.transfer`. Added or changed:

- `Member`: `suspect(now, from: Colo, about: SiteId, term) -> Ready<C, Verdict>`,
  `transfer(now, to) -> Ready<C, Result<TransferId, TransferError>>`, `transferring() -> Option<(SiteId, TransferId)>`,
  `call_failed(now, CallFailure) -> Ready<C, ()>`, `confirm_begin(now) -> ConfirmId`,
  `confirm_end(now, ConfirmId) -> Ready<C, ()>`, `set_drained(now, sites) -> Ready<C, ()>`,
  `clear_no_lead(now) -> Ready<C, ()>`. `campaign` keeps its signature and also returns `Verdict::MayNotLead` on a
  drained member or one under `no_lead_until`.
- `Envelope::failure(deadline: bool) -> Option<CallFailure>`.
- New types: `CallFailure`, `FailedCall`, `ConfirmId`, `StepDownWhy`, `Colo`, `TransferError`, `Health`, and, in
  `edge`, `Receipt<C>` (`new`, `id`, `target`, `call`, `missed`, `not_leader`, `verdict`, `saw_term`) and `Suspect`.
  Constants `STEP_DOWN` and `SUSPECT`.
- `Message` gains `TimeoutNow`, `TransferRefused`, `Probe`, `ProbeRes`, `Ping`, and `Pong`.
- `AppendReq.transfer` is `Option<(SiteId, TransferId)>`, was `Option<TransferId>`. `AppendRes` gains `boots`.
- `Alarm` gains `NoLead`, `OwnDefect`, and `StepDown`; `Deny` gains `Drained` and `Suspected`; `Verdict` gains
  `MayNotLead`; `MemberState` gains `transfer_at`.

BF-14 is structural: no `Member` method takes a command for another member, and `Receipt` exposes its command only
through `call()`, which names its one target. A miss (`missed`) returns `Suspect` reports and leaves the target
unchanged; only a `NotLeader` answer (the target holds no ticket) or a member answering a report as leader moves it.

## Simulator

`crates/raft/tests/common/sim.rs` gained a host that times calls and confirms, and three gray faults on one site at
a time, as in Milestone 0's B0-06:

- **Calls**: 30% of member calls to or from the site fail fast with an error, and so do edge receipts from it.
- **Slow confirms**: 30% of the site's storage confirms take 1 s, and the site's outputs wait for each confirm (the
  output gate). Confirms complete in order.
- **Failing writes**: 30% of the site's inputs that write fail, which resets the member before anything leaves.

The host reports each confirm's end and each call whose deadline passes with no reply. Edges are now `Receipt`s: a
write's command goes only to the receipt's target, a receipt with no answer by COMMIT_HOLD + CALL_DEADLINE is hedged
with `suspect` reports to the other members, and every report names the member it is about and the edge's colo.

`Faults::GRAY` runs the three faults (one at a time, 5 to 30 s each, 0.2% per step) with partitions, duplicates,
reordering, and 1% loss. It has no crashes beyond those the failing writes cause: at HARSH's crash rate nearly every
member is inside RESET_QUARANTINE by its two boots, so every trip holds and no `STEP_DOWN` transfer runs. HARSH still
runs alone, with every M1-WP2 check.

Every safety check of M1-WP2 runs after every message and step in both fault models, plus one more: BF-14, no command
sent to a member while its receipt was outstanding at another. `long_sweep` now runs the same seeds under HARSH and
then under GRAY.

| Run | Seeds × steps | Result |
| --- | --- | --- |
| Default `cargo test`, HARSH | 60 × 8,000 | Pass, 0 forwarded |
| Default `cargo test`, GRAY | 30 × 8,000 | Pass, 0 forwarded, 20 `STEP_DOWN` transfers, 111 holds |
| `long_sweep`, release, HARSH | 2,000 × 10,000 | Pass: 20,096,795 steps, 206,541 operations (76,001 acked, 30,087 refused, 80,725 reads, 19,728 with no reply), 224,079 crashes (97,596 mid-input), 31,180 partitions, 3,866 leaders, 0 forwarded, 0 `STEP_DOWN` transfers, 11,335 holds |
| `long_sweep`, release, GRAY | 2,000 × 10,000 | Pass: 20,071,357 steps, 2,255,722 operations (961,920 acked, 391,108 refused, 902,620 reads, 74 with no reply), 17,285 gray faults, 24,996 write failures (each a reset), 15,297 partitions, 12,510 leaders, 0 forwarded, 1,338 `STEP_DOWN` transfers, 11,343 holds |

The long sweep took 242 s for both halves (4 min 17 s with the release build).

## Liveness against Milestone 0

`crates/raft/tests/liveness.rs`. Three members with one-way delays of 5 to 150 ms and no other fault, 16 edge
clients (about 14.5 acked writes a second on the group; Milestone 0 ran 25 per group, 100 writes/s over 4 groups),
30 s of warm-up, then the fault on the leader's site for 60 s. "New leader" is the first term led off the site, which
is how Milestone 0 timed a move (first sample with a leader off A). "Healthy leader commits" is the first client write
acked by a leader off the site. Default `cargo test` runs 12 seeds per fault; the table is the ignored
`long_liveness`, 100 seeds per fault, release build.

| Fault on the leader's site | New leader off the site, s (min / p50 / p90 / max) | Healthy leader commits, s | Moved by `STEP_DOWN` | Leader changes per run | Milestone 0 (B0-06, `min3-` and final code) |
| --- | --- | --- | --- | --- | --- |
| 30% of calls fail | 2.3 / 5.1 / 8.4 / 12.0 | 2.7 / 5.4 / 8.8 / 12.6 | 100 of 100 | 1 in all 100 | 2.9 to 4.1 s, 1 change each (`min3-calls`); 2.4 to 5.2 s on the design rule |
| 30% of confirms take 1 s | 6.7 / 10.4 / 12.6 / 21.9 | 7.0 / 10.8 / 12.9 / 22.5 | 99 of 100 | 1 in all 100 | 2.8 to 4.2 s (`min3-slow`); 3.9 to 6.1 s with the count at 6 (`f35q`) |
| 30% of writes fail | 10.4 / 11.5 / 14.1 / 17.8 | 10.6 / 11.8 / 14.4 / 18.2 | 0 of 100 (election) | 1 in all 100 | 4.9 to 13.3 s by election (`dr-writes`) |
| None, 60 s | not moved | | | 0 in 12 | 0 in 10 minutes (`f35q-healthy1`) |

- **Failing calls** moves later than Milestone 0 at the median (5.1 s against 2.9 to 4.1 s). A leader judges the rule
  only once every follower has at least `commit_fail_min` (3) failed calls in the window, and this load gives each
  follower about 4 calls a second, so at 30% the third failure comes at about 2.5 s. Then the probe runs 20 pings in
  batches of 5, which takes up to 4 round trips, and the transfer waits for the other follower's ack. Milestone 0 ran
  25 writes per group per second and logged 3 to 6 failures per follower at its trips. The p90 of 8.4 s and the max
  of 12.0 s are trips held once by the probe (a probe ping crossing the sick site fails) and moved on the next window.
- **Slow storage** is slower than Milestone 0 by about 6 s at the median, and 1 run of 100 went past the 20 s target
  (21.9 s to a new leader; 2 of 100, seeds 2028 and 2048, past 20 s to the first healthy commit, so `long_liveness`
  fails on them). The cause is the confirm rate, not the
  rule: the sick leader confirmed 2.0 writes a second here (one per entry, one entry in flight, and every slow confirm
  holds the next), so the sixth slow confirm (`confirm_slow_min`) comes at about 10 s. Milestone 0's leaders confirmed
  many more writes a second at 25 writes per group, and its trips read "6 of 22 to 33". With twice the clients (26.8
  acked writes/s, 2.0 confirms/s, `RAFT_LIVE_CLIENTS=32`) the p50 was 10.8 s and the max 21.1 s: the confirm rate, not
  the write rate, sets it. This matches the design's own caveat for `commit_fail_min` ("a quiet group needs about 10
  calls per follower a window"), which applies to `confirm_slow_min` too and is not stated for it.
- **Failing writes** moves by election, as in Milestone 0: the reset member cannot trip.
- **No fault**: 0 trips and 0 leader changes in 12 runs of 60 s under load.

**Follow-on leader changes.** Milestone 0 had 0 follow-on changes in 90 leader kills with `commit_fail_min` (2 of 90
without it). Here, 1 change in each of the 300 runs of `long_liveness`, and the
targeted kill case `a_new_leader_does_not_count_a_follower_it_has_not_heard_from` has 0. The simulator was not run
as a 90-group kill campaign.

## Targeted cases

`crates/raft/tests/leadership.rs`, 32 cases. Each pins one rule; where the design cites a past failure, the case
reproduces it.

Transfer:

- `transfer_hands_leadership_to_a_caught_up_target`: every AppendEntries during the transfer carries `(to, id)`, no
  ack to those rounds promises, the target wins `term + 1`, and a host-asked transfer sets no `stepped_down_at`.
- `transfer_waits_for_the_other_follower_so_the_target_wins_without_the_old_leader`: **B0-07's 40 s leaderless
  group.** C's appends are lost for 1.5 s and B's vote request to A hangs throughout. No `TimeoutNow` leaves before C
  accepts a round carrying the id; then B wins on C's vote alone.
- `a_voter_that_has_not_seen_the_transfer_refuses_the_target_as_promised`: the control for the case above.
- `only_the_accepted_transfer_id_breaks_a_promise`: no id, or another id, is refused `Promised`; the accepted id is
  granted.
- `an_unreachable_target_is_retried_until_the_lease_then_abandoned`: still running at LEASE - 0.5 s, abandoned by
  LEASE + 0.5 s, every later round carries `transfer: None`, and a read at abandonment waits for a round.
- `a_refused_transfer_is_abandoned_at_once_and_leaves_no_lease`: a drained target refuses (`TransferRefused`) and the
  leader abandons within one round trip, with no lease from before the transfer.
- `a_target_that_returns_within_the_lease_still_takes_over`: catch-up is retried past COMMIT_RETRIES.
- `a_transfer_vote_promises_the_target`: see "Bugs the tests found".
- `a_late_timeout_now_after_abandonment_is_ignored`.

`suspect`:

- `two_colos_within_the_window_stop_promises_to_a_renewing_leader`: two colos 10.2 s apart do not stop promises;
  two within 10 s do, while the leader keeps renewing; reports about a member that is not the follower's leader count
  for nothing.
- `promises_return_after_twice_the_window`: with both followers withholding the leader serves reads only after a
  round (no lease), and promises return after 20 s.
- `a_higher_term_restores_promises_but_pre_vote_still_refuses_the_reported_leader`: the new leader's first append is
  promised inside the window, while the old leader's pre-vote is refused `Suspected` until 20 s after the reports.
- `one_report_starts_a_campaign_once_the_promise_has_lapsed`.
- `not_leader_naming_no_leader_is_reported_to_that_member_too`: a deposed leader with the newest log wins the next
  term through the reports its own `NotLeader` caused.

`Health` and `STEP_DOWN`:

- `commit_errors_trip_and_the_probe_picks_a_healthy_target`: 30% failing calls on A trip with at least 3 failures and
  over 5% to each follower; A transfers, and records `stepped_down_at`.
- `fewer_than_commit_fail_min_failures_never_trip`: a 20% rate on 2 failures a window, 30 s, no trip.
- `six_slow_confirms_trip_and_five_do_not`: five 700 ms confirms never trip; six do, with no probe.
- `deadline_misses_during_the_leaders_own_stall_are_excused`: **LF-12.** One 3.5 s confirm on the leader with every
  call in it missing its deadline: no commit-error trip. The control reports the same calls as errors, not misses,
  and trips.
- `a_new_leader_does_not_count_a_follower_it_has_not_heard_from`: **the leader kill.** B wins while A is down for
  15 s; every call to A fails and C misses 3: no trip.
- `a_stepped_down_member_is_not_a_target_so_healthy_sites_do_not_ping_pong`: **two healthy sites passing a group
  back and forth (F35, `f35-slow3`, 6 and 11 changes).** A leaves on slow confirms to B; calls into A fail 40% and
  the B-C link 12%, so B moves to C on commit errors; C then trips and holds every window for 60 s, because B is
  quarantined. Exactly two moves.
- `the_probe_picks_the_healthy_site_never_the_sick_one`: **the transfer onto the sick site (`tgt-writes1`, B0-33
  p4r4).** B's calls fail to A and just over 5% to C. The probes read `c>a` high and `a>c` low, and B moves to C.
- `a_sick_prober_makes_the_leader_hold`: A fails both ways, so A's probe of C fails and B holds (`dr-calls1` `-g2`).
- `the_leader_holds_when_no_probe_qualifies_a_target`: every link fails 30% (the June 2025 class): 2 trips in 15 s,
  both held.
- `a_healthy_leader_never_trips`: 60 s under load.

`home`, drain, and `led_term`:

- `leadership_returns_home_after_it_recovers_and_catches_up`: B leads while A is down; A back but failing 20% of calls
  gets no transfer for 15 s; A healthy gets it within the next 15 s.
- `a_quarantined_home_gets_no_transfer`.
- `a_resumed_led_term_still_goes_home`.
- `a_drained_site_hands_off_and_takes_no_leadership`: hand-off within one LEASE to the one undrained follower,
  `Target` for a drained one, pre-vote `Drained`, and `MayNotLead` on a report.

Refusals:

- `an_older_refuser_makes_the_leader_truncate_and_hand_off`: release 2 leader, release 1 followers that cannot read
  the format. The leader truncates the refused entry, persists `no_lead_until`, answers the ticket `NotLeader`, and a
  follower leads; a restart on release 3 clears the bar.
- `both_same_release_refusers_are_the_leaders_own_defect`: truncate, `OwnDefect`, keep leading, commit the next
  entry.
- `a_same_release_refuser_of_a_committed_entry_cannot_follow_until_its_release_changes`.

BF-14:

- `bf14_a_missed_receipt_is_reported_never_forwarded`: five misses in a row each return reports to B and C about A,
  and the receipt's one target stays A; a `Promised` verdict does not move it, a `Leader` verdict does.
- Every simulator run counts commands sent to a member while the receipt was outstanding at another: 0 in every run.

## The tests catch real bugs

Seventeen bugs were planted one at a time, and each failed at least one test (`cargo test -p graftyard-raft`, all
targets):

| Planted bug | Caught by |
| --- | --- |
| No wait for the other follower before `TimeoutNow` | `transfer_waits_for_the_other_follower_…` |
| Any transfer id breaks a promise | `only_the_accepted_transfer_id_breaks_a_promise`, `a_voter_that_has_not_seen_the_transfer_…` |
| Acks to transfer rounds promise | `transfer_hands_leadership_to_a_caught_up_target` |
| A transfer or its abandonment keeps the old lease | `a_refused_transfer_is_abandoned_at_once_and_leaves_no_lease` |
| `commit_fail_min` ignored | `commit_errors_trip_and_the_probe_picks_a_healthy_target` |
| Own-stall exclusion removed | `deadline_misses_during_the_leaders_own_stall_are_excused` |
| Heard-from rule removed | `a_new_leader_does_not_count_a_follower_it_has_not_heard_from` |
| `stepped_down_at` ignored for `STEP_DOWN` targets | `a_stepped_down_member_is_not_a_target_…` |
| Probe result ignored | `the_probe_picks_the_healthy_site_…`, `a_sick_prober_makes_the_leader_hold`, `the_leader_holds_when_no_probe_…` |
| `confirm_slow_min` ignored | `six_slow_confirms_trip_and_five_do_not`, `deadline_misses_during_…` |
| A higher term does not restore promises | `a_higher_term_restores_promises_…`, `leaders_leave_a_site_whose_writes_fail` (liveness) |
| Pre-vote does not refuse a suspected leader | `a_higher_term_restores_promises_but_pre_vote_still_refuses_…` |
| One colo is enough | `two_colos_within_the_window_stop_promises_…` |
| An older refuser treated as `cannot_follow` | `an_older_refuser_makes_the_leader_truncate_and_hand_off` |
| BF-14: a hedge's answer moves the command | `bf14_a_missed_receipt_is_reported_never_forwarded` |
| A transfer vote does not promise the target | `a_transfer_vote_promises_the_target` |
| No transfer back to `home` | `leadership_returns_home_…`, `a_resumed_led_term_still_goes_home` |

Five of these first passed every test: the wait before `TimeoutNow`, acks to transfer rounds promising, the old lease
kept, the heard-from rule, and the quarantine target rule. Each test was then rewritten until it failed on the plant
and passed without it; the table is the final run. `a_transfer_vote_promises_the_target` failed on its plant only
through an assertion on state at first, and was changed to fail on the behavior (the old leader starting a new term).

## Bugs the tests found

In the crate:

1. **A transfer deposed its own target.** After a transfer vote, the other follower's promise still named the old
   leader, which a promise does not bar. A `suspect` report to the old leader before the target's first append
   reached it made the old leader campaign, win that follower's vote, and depose the target; the group then had no
   leader (liveness, calls seed 108). A transfer vote now promises the target.
2. **Late reports withheld promises from the new leader.** A `NotLeader` from the target while its transfer
   campaign ran, reported by edges to the other follower, counted against that follower's new leader, because the
   follower kept the old term's `leader` across the term change. Three changes in one run (calls seed 1039). A higher
   term now clears `leader`, and a report counts only about the member the follower promised and follows.
3. **A gray leader's rounds cancelled every pre-vote against it.** While `suspect` withheld promises, each append from
   the still-renewing leader cancelled the follower's pre-vote, so the follower could not campaign until the leader
   stopped (9 s of cancelled pre-votes, slow seed 2023). A pre-vote now survives appends while promises are withheld.
4. **A commit-path call retried only on `tick`.** A call failed fast with an error waited for the next tick past
   CALL_DEADLINE, which made the commit-error rule slow to collect failures. It now retries at once up to
   COMMIT_RETRIES times in a row, as the design says.
5. **Probe requests were not retried**, so one lost `Probe` or `ProbeRes` disqualified every target (calls seed
   1047, held while both probes read 0.00). The leader re-asks after CALL_DEADLINE, and a prober answers a repeat from
   its last result.
6. **The transfer back to `home` waited for `home` to hold the last index.** Under load an entry is always in flight,
   so this held for about 58 s with no fault. It now asks for every committed entry; the transfer itself brings
   `home` to the last index.

In the simulator: a reply could fail independently of its request (two rolls per call, so a 30% call fault failed
51%); confirms were serialized end to end instead of overlapping; and calls to a reset member returned at once instead
of missing their deadline.

## Where the code departs from the design

Each is quoted against the design sentence it changes.

- "A follower that gets reports from two colos within 10 s stops granting promises to a leader that is still
  renewing" (Shape, Leadership, `suspect`). `suspect` takes `about: SiteId`, the member whose receipt missed, and a
  report counts only when `about` is the leader this follower promised and has heard from in the current term. The
  design's signature is `suspect(follower, from: Colo, term)`; without `about`, a report about a member that is not the
  follower's leader withholds promises from the real one (bug 2 above).
- "a voter disregards its promise to the old leader only for a transfer vote that names the `transfer_id` of the last
  AppendEntries it accepted" (Shape, Leadership, `transfer`). Kept, and in addition a granted transfer vote is a promise
  to the target (bug 1). The old leader also grants a transfer vote to its own target for its own transfer id.
- "While it runs, every AppendEntries carries `transfer: Some((to, transfer_id))`". `MemberState.transfer` stores
  only the id, as the design's row says, plus `transfer_at`, the leader's send time of the append it came from, so a
  delayed append from before an abandonment cannot bring the id back after a `transfer: None` round. `MemberState`
  gains `transfer_at`.
- "A transfer that has not completed within one LEASE is abandoned". Also abandoned at once when the target answers
  `TransferRefused` (it is drained, under `no_lead_until`, or quarantined), a message the design does not have.
- "On errors, it first asks the other follower to ping each candidate target". The probe is `Probe`, `Ping`, `Pong`,
  and `ProbeRes` messages through the member, 20 pings in batches of 5 as Milestone 0's `linkErrors`. The leader
  waits 4 x CALL_DEADLINE + 0.5 s for the answers, as Milestone 0 did.
- "Among those it takes the one with the fewest failed calls, then the most caught up". Fewest failed calls in the
  window, by count, from the leader.
- "A leader transfers when … its group's `home` has recovered and caught up". "Recovered" is not defined. Here: at
  least 5 calls to `home` in the last `STEP_DOWN` window with none failed, `home` outside RESET_QUARANTINE, the
  leader not tripped, and at least one window since the leader started [guess]. "Caught up" is `home` holding every
  committed entry. After an abandoned transfer to `home` the next try waits 60 s [guess].
- "Pre-vote and transfers refuse a candidate whose two `boots`, or whose `stepped_down_at`, fall inside it." A
  transfer target also refuses itself when inside its own quarantine, and `AppendRes` gains `boots` so the leader
  can see a follower's boots.
- "If both followers on the leader's own release refuse one uncommitted entry, the leader treats it as its own defect,
  truncates the uncommitted tail, and alarms." A refusal of an uncommitted entry is classified once its round settles:
  the entry commits, every follower refused it, or CALL_DEADLINE x (1 + COMMIT_RETRIES) passed, as Milestone 0 did.
- "persist `no_lead_until`, which clears when A's release changes or the `Release` group records a rollback of the
  refusing site". It clears at `restore` when the release differs; the rollback is the host's call to
  `clear_no_lead`. While it holds, the member takes no command, resumes no `led_term`, takes no transfer, and does not
  campaign until it has seen no leader for 2 x ELECTION_AFTER.
- Drain: the design has drained members refuse to campaign "while they keep voting and storing". Here a drained
  member also refuses a pre-vote for a drained candidate (`Deny::Drained`) unless the voter has seen no leader for
  2 x ELECTION_AFTER, mirroring the quarantine waiver.
- "Campaigns are request-driven through this call, never alarm-driven". Kept. The leader's `STEP_DOWN`, drain, and
  home checks run from `tick`, which the host drives; they start transfers, never campaigns.
- `Health` is stated as a struct; `STEP_DOWN` here is the same values.

## Design problems found

1. **`confirm_slow_min` on a quiet or slow-storage group.** At 6 slow confirms a window, a leader whose every write is
   slow but which confirms 2 writes a second (one entry in flight, each slow confirm holding the next) trips at about
   10 s and, in 2 of 100 runs here, misses the 20 s target. The design states the matching caveat for
   `commit_fail_min` ("a quiet group needs about 10 calls per follower a window") but not for the confirm rule. The
   gray-region target "every group moved within 20 s" may not hold for slow storage on quiet groups [inference from
   the simulator].
2. **`suspect` needs to say which member a report is about.** The design's `suspect(follower, from, term)` cannot
   tell a report about the follower's leader from one about a member that just answered `NotLeader`. Milestone 0's
   prototype also passed a `target` (`tests/m0/src/raft/member.ts`, `suspect`).
3. **A transfer vote must hand the promise to the target**, or the deposed leader can win its group back on the next
   report (bug 1). The design's rule covers only when a voter may disregard its promise.
4. **"Recovered" for the transfer back to `home` is not defined**, nor how soon to retry after one fails.
5. **The design's `Verdict`, `TransferError`, `SuspectError`, and `Colo` are named but not defined**; the crate's
   versions are listed under the public API changes.
6. **Under HARSH's crash rate every `STEP_DOWN` holds**, because both followers are inside RESET_QUARANTINE by two
   boots. That is the design working as written (a flapping member should not lead), but it means a group whose sites
   all reset often cannot move a gray leader by `STEP_DOWN`, only by `suspect` and election.

## Linearizability

- Both halves of the long sweep pass the crate's Wing–Gong checker, per key, at the end of every seed.
- The histories exported with `RAFT_HISTORY` (the HARSH half at `path`, the GRAY half at `path.gray`) were checked by
  the Milestone 0 checker (`tests/m0/porcupine`):
  - HARSH: `{"droppedFailedReads":1859,"noReplyWrites":17869,"operations":204682,"result":"Ok"}`
  - GRAY: `{"droppedFailedReads":8,"noReplyWrites":66,"operations":2255714,"result":"Ok"}`
  - The HARSH history with one read changed to a value nobody wrote (`nobody-wrote-this`):
    `{"droppedFailedReads":1859,"noReplyWrites":17869,"operations":204682,"result":"Illegal"}`

## Checks

- `cargo test -p graftyard-raft`: 35 s wall. Unit 0; `leadership` 32 passed; `liveness` 4 passed, 1 ignored (9.5 s);
  `scenarios` 21 passed; `sim` 5 passed, 1 ignored (25.9 s).
- `RAFT_SEEDS=2000 RAFT_STEPS=10000 cargo test -p graftyard-raft --release --test sim -- --ignored --nocapture`:
  `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 5 filtered out; finished in 242.27s`.
- `RAFT_LIVE_SEEDS=100 cargo test -p graftyard-raft --release --test liveness long_liveness -- --ignored --nocapture`:
  fails on the two slow-storage seeds over 20 s (`slow seed 2028: moved in 22462 ms`, `slow seed 2048: moved in
  20951 ms`), as described under "Liveness against Milestone 0"; every other check in it passes.
- `cargo build -p graftyard-raft --target wasm32-unknown-unknown --release`: finished.
- `cargo clippy -p graftyard-raft --all-targets -- -D warnings`: clean. `cargo clippy --workspace --all-targets`:
  finished, with warnings only in `crates/git` (test `shard`) and `tests/m1/wp4`, which this package does not own.
- `cargo fmt -p graftyard-raft --check`: clean. `cargo fmt --check` on the workspace lists diffs only in `crates/git`
  and `tests/m1/wp4`, `tests/m1/wp5`.
