# B0-02 follow-up: a far voter that answers late keeps its pipeline, and each link sizes its own calls (2026-11-13)

The operator's decisions on `carried-8.md`'s open item (r300-3, C20): fix it as a follow-up branch off main
`82a0bf9`, so the B0-02 gate on `6d2ab81` stays valid. Stop pipelining only on calls that are lost, not on late
answers; keep F67 rule 13's `link_lossy` as it is; give each link a call bound that halves on a miss and grows back
on answers, floored at one whole entry and capped at `APPEND_BYTES_MAX`; model the far link in the simulator as
measured, with in-flight independence tagged [inference]; test weur leading, enam lost, apac catching up within a
stated time; then the sweep, Porcupine, plants, one hosted check of the scenario and the 16-run B0-02 gate. Branch
`m0-adaptive-append`; install `graft-m3d`.

## Verdict

**B0-02 still passes on this tree: 16 of 16** (`78f0749`; `graft-m3d`, standard-4, 2026-11-13). Commit p99 was 253 to
543 ms, at 280 commands/s for the 300 offered runs and 306 to 308 commands/s for the 330 runs. Heavy turns stayed at
`carried-8`'s level: 30 `/commit` turns of 80 ms or more against 28, and CPU p999 18 to 45 ms.

**The r300-3 shape is fixed in the simulator.** weur leads and enam is lost at 30 s, and apac takes over the quorum
with commits stalled for at most 20 s. That held on 24 of 24 seeds (20 to 43), median 12.6 s and worst 16.6 s. On main
`82a0bf9` it held on 0 of 24, with apac a mean 149 entries behind when enam stopped.

**Hosted, the scenario stalled 0.8 s** with 0 failed commands (`m0c9-far2`). apac's link was healthy in that run, so
the slow-link catch-up is shown in the simulator only.

The long sweep (live half 4,000 seeds at an absolute 30 s), Porcupine, plants 9 (12 of 12) and 8 (15 of 16, the
accepted miss) and the full checks pass. One `long_liveness` writes-fail seed is over its bound. Main has the same
tail on another seed (14011), from a three-term split vote; it is recorded below and is the next branch's fix.

## What changed (`crates/raft`)

- **Pipelining stops only on lost calls (`pipe_lossy`).** A voter's call counts against its pipeline when it fails
  with an error, or misses `CALL_DEADLINE` and stays unanswered for `LATE_LIMIT` (10 s) more. A call that misses the
  deadline and then answers does not count. The rule is otherwise `link_lossy`'s: over `commit_errors` of the
  window's calls, at least `commit_fail_min` of them. F67 rule 13 keeps `link_lossy`, which counts a late answer as a
  loss, so hand-off targets are picked as before. `LATE_LIMIT` moves from `workers/site/src/host.rs` into
  `crates/raft`.
- **A per-link call bound (`Lead::bound`).** It starts at `APPEND_BYTES_MAX` for each follower. A lost call halves it,
  once per cut. A call answered within `CALL_DEADLINE` adds back `APPEND_BYTES_MAX / 8`. A call always carries one
  whole entry, so at 0 it carries exactly one.
- **A reordered pipelined call waits behind the call ahead.** A call that reached the follower before the call ahead
  of it is refused for the gap. The leader now drops that call and those after it from the pipe, keeps the calls ahead,
  and pipelines again when the call ahead answers. Before, it cleared the whole pipe and started the voter again from
  `next`.
- **A voter behind gets calls until `PIPELINE` are out**, each within its bound, rather than one call per answer.

The operator's first rule halved the bound on every missed `CALL_DEADLINE`. The simulator showed that this rule
lost to halving only on lost calls (below), so the bound halves on a lost call. The reorder rule and the fill were not
in the decision. The simulator found both: without them the far voter still fell behind (below).

## The far link in the simulator

`crates/raft/tests/common/load.rs` models a link from the leader to one follower (`Load::far`) from r300-3's own
trace (`tests/m1/wp2/raw/m0c8h-r300-3.stats.json`, the weur leader's `AppendQueued` and `AppendEnded` rows to apac,
163 calls of 20 KB or more):
- Each AppendEntries costs the link's one-way delay plus its bytes times a cost per KB. The cost is drawn per call
  from that trace's distribution of (call ms - 255 ms RTT) / KB, at each twentieth: p10 1.2, p50 2.6, p70 5.9, p80
  8.7, p90 12.6, p95 16, max 87 ms per KB [measured].
- The draw does not depend on how many calls are in flight. In the trace, per-call ms per KB had p50 6.9, 7.2, 7.3 and
  6.4 at 1, 2, 3, and 4 or more calls out [measured], so the link is modelled per call rather than as shared bandwidth
  [inference, until a hosted run shows it].
- Calls that leave close together can arrive out of order, as the trace's did: of calls issued within 50 ms of each
  other, 27% ended in the other order in r300-3 and r330-6. The model reorders more: 55% within 1 ms and 44% at 50
  to 150 ms, against 28% and 6 to 28% hosted. So it is pessimistic on reordering.
- An AppendEntries with no answer by `CALL_DEADLINE` is reported failed, and an answer after that is still stepped,
  as the host does within `LATE_LIMIT`.
- A stopped member gets and sends nothing.

The r300-3 scenario (`a_far_voter_keeps_up_so_losing_the_near_one_stalls_commits_under_20_s`,
`crates/raft/tests/admission.rs`):
- weur leads; apac is on the far link; 300 commands/s of about 1 KB, about 290 KB/s to each follower, as r300-3's
  leader wrote.
- After 30 s, enam's member stops, so only apac can make the quorum.
- The stall runs from the stop until the first command in an entry appended after the stop is acked.
- Pass: the stall is at most 20 s, with no leader change. The kept test runs seeds 12 and 13; `R300_SEEDS=a..b` runs
  more.

Over seeds 20 to 43 (24 seeds) [measured, in the simulator]:

| Tree | Seeds under 20 s | Median stall | p90 | Worst | Mean entries apac was behind at the stop |
|---|---|---|---|---|---|
| main `82a0bf9` | 0 of 24 | none: no commit within 30 s | none | none | 149 |
| this branch | 24 of 24 | 12.6 s | 15.3 s | 16.6 s | 60 |
| without the reorder rule | 16 of 24 | 18.5 s | 24.1 s | 29.6 s | 104 |
| without the fill | 21 of 24 | 14.5 s | 20.8 s | 21.9 s | 65 |
| halving on every missed `CALL_DEADLINE` | 7 of 24 | 23.9 s | none | none | 108 |

On main, apac falls further behind all run and commits do not resume within 30 s in 23 of 24 seeds. That matches
r300-3's 219 entries and C20's estimate of 40 to 70 s. Halving the bound on every miss loses because the link's
misses come from its tail at any call size: in r300-3, calls of 25 to 50 KB took 1 s or more about as often as calls of
75 to 100 KB (2 of 19 against 10 of 70). So smaller calls do not miss less, and each one costs another round trip.
Halving on a lost call keeps the bound for a link that really drops calls. On the hosted gate it never halved (below).

## Kept tests and plants

Kept tests (`crates/raft/tests/scenarios.rs` unless named):
- `a_voter_is_pipelined_unless_its_calls_are_lost` replaces `a_voter_is_pipelined_unless_its_link_is_lossy`. One
  error keeps the pipeline (4 calls). `commit_fail_min` errors stop it (1 call). As many late answers keep it (4).
  As many misses unanswered past `LATE_LIMIT` stop it (1).
- `a_voter_whose_calls_arrive_out_of_order_keeps_its_pipeline`: calls delivered newest first, with no tick, still
  bring the voter level, with at most one extra call per entry.
- `a_voters_call_bound_halves_on_lost_calls_and_grows_back_on_answers`: lost calls cut the bound to one entry a
  call. Answers in time bring it back past half of `APPEND_BYTES_MAX`, and the voter catches up past an entry of
  300 KB, three times `APPEND_BYTES_MAX`, because a call always carries one whole entry. With
  `a_lagging_follower_catches_up_in_calls_under_the_byte_cap` (entries of 400 KB, no empty call), this covers the
  floor.
- `a_far_voter_keeps_up_so_losing_the_near_one_stalls_commits_under_20_s` (`crates/raft/tests/admission.rs`):
  seeds 12 and 13 in the kept run, and 20 to 43 above.

Plants (`tests/m0carried/plant9.py`, one mutant at a time, all raft suites in release): 12 of 12 caught.

| # | Plant | Caught by |
|---|---|---|
| 0 | Pipelining stops on late answers (`link_lossy`) | r300-3 scenario |
| 1 | Pipelining never stops | `a_voter_is_pipelined_unless_its_calls_are_lost` |
| 2 | A late answer still counts as lost after `LATE_LIMIT` | r300-3 scenario |
| 3 | An unanswered miss never counts as lost | `a_voter_is_pipelined_unless_its_calls_are_lost` |
| 4 | A miss counts as lost at once, not after `LATE_LIMIT` | r300-3 scenario |
| 5 | The bound never halves | `a_voters_call_bound_halves_on_lost_calls_and_grows_back_on_answers` |
| 6 | The bound never grows back | same |
| 7 | The bound halves on every miss | r300-3 scenario |
| 8 | A call over the bound carries no entry | B0-29, B0-33 and others in `admission.rs` |
| 9 | A reordered call clears the pipe | r300-3 scenario |
| 10 | A reordered call is resent at once | `a_higher_term_restores_promises_but_pre_vote_still_refuses_the_reported_leader` (resends without end) |
| 11 | A voter behind gets one call per answer | r300-3 scenario |

`m0-carried-8`'s plants (`plant8.py`) on this tree: 15 of 16. The same miss as on `carried-8`'s tree, the entry cap
back at 60 KB, which the operator accepted there.

## Sweep, Porcupine, liveness and checks

On `7f48047` (the tree the site day built, plus a `mut` clippy flagged in a test):
- **Long sweep** (`RAFT_SEEDS=2000 RAFT_STEPS=10000`): passes. HARSH 2,000 seeds, GRAY 2,000, live 4,000 from 10000
  at the absolute 30 s bound. The longest stretch with a healthy quorum, requests waiting, and none completing was
  16.5 s (HARSH, seed 11180), 24.7 s (GRAY, seed 11595), and 23.6 s live (seed 10545).
- **Porcupine** (`go run . -seg 2000`): Ok on all three histories. 168,526, 2,226,306 and 6,065,525 operations.
- **`long_liveness`** (60 seeds a fault): calls, slow confirms and loop lag pass, but writes fail missed one seed:
  3052 moved in 21.9 s, against 10.9 s on main.
  - The cause is a split vote. Sites 1 and 2 campaign together and refuse each other with `Voted` for three terms
    (42.9, 46.6 and 50.0 s into the run) before one backs off. The new rules play no part in the split vote itself: the
    reorder rule changes when the group's messages leave, which moves this seed into a split vote, as it moves others
    out of one.
    Without the reorder rule, the seed moves in 10.9 s.
  - Over 20,000 writes-fail seeds (3000 to 22999), main and this branch had the same p50 (11.6 s), p99 (15.6 s) and
    count of seeds at 17 s or more (27 and 26). Each had exactly one seed over 20 s, the same one: 14011, at 21.5 s.
    On main, 14011 is also a split vote, sites 1 and 2 refusing each other with `Voted` for three terms.
  - So writes-fail's 20 s target misses one seed in about 20,000 on both trees. This branch's 3052 is that tail
    landing in the 60-seed window, not a regression. It is recorded here and in C20 rather than hidden by a
    different seed range.
- **Full checks**: pass. fmt, workspace clippy, wasm32 clippy for site and edge (with and without `test-faults`) and
  ops, `cargo test --workspace` (841 passed, 0 failed), `go test ./...` in `containers/git/server`, and the three
  `scripts/check_*.py`.

## Hosted check: weur leads, enam is lost

Site day 21:48 to 21:56Z on `78f0749`, `graft-m3d`, `SHARD_INSTANCE=standard-4`, image `sha256:15f6ed1b…`; the
bench edge was redeployed with a `home` parameter and a `/member` route (`tests/m1/wp2/edge.ts`). Driver:
`tests/m0carried/far9.ts`.
- A new group homed on weur (site 1). Creating it with that home keeps weur as leader; a `/transfer` to weur would
  move back to enam's home within one `STEP_DOWN` window. The group led from weur at term 2 for the whole run.
- 300 commands/s of 1 KB for 120 s. At +60 s, enam's member (site 0) was severed (`/sever`) for the rest of the run,
  so apac (site 2) made every quorum after that. A poller read every member's `/status` once a second.
- `m0c9-far1`: the run itself completed, but the driver could not write its row (the deploy checkout had no `raw/`
  directory), so its numbers are only in the log: 0 failed of 36,000, commit p99 by 15 s chunk 748 to 917 ms before
  the sever and 1,578 to 2,096 ms after. Fixed, then rerun as `m0c9-far2`.

`m0c9-far2` (`tests/m0/results/raw-carried-9/m0c9-far2.far.json` and `.stats.json.gz`; `m0c9-far1.log`) [measured]:

| | Before the sever | After the sever |
|---|---|---|
| apac behind the leader (1 s polls) | mean 1.7, max 6 entries | mean 0.7, max 1 |
| Calls to apac, ms p50 / p99 / max | 209 / 686 / 985 | 222 / 884 / 1,074 |
| Calls to apac that missed `CALL_DEADLINE` | 0 of 454 | 1 of 323 (answered late, 1,074 ms) |
| Client commit p99 by 15 s chunk | 672 to 801 ms | 876 to 1,630 ms |
| Failed commands | 0 | 0 |

- **Commit stall: 0.8 s.** The first poll after the sever already showed commits past the leader's last index at the
  sever. apac was 1 entry behind when enam went, so there was no backlog to take.
- **This run did not reproduce r300-3's link.** apac's calls answered at p99 686 ms before the sever, with no miss in
  454. r300-3 had 6 misses in 32. So the hosted run shows the stall after losing the near voter on a healthy far link,
  1 s, but not the slow-link catch-up the simulator test covers. That remains [measured, in the simulator] only.
- **Calls in flight [measured, this run].** Per-call ms per KB to apac, for calls of 20 KB or more, by calls out when
  the call was issued: p50 3.0 at 1 (n 259), 4.2 at 2 (222), 3.9 at 3 (115), 4.1 at 4 (153). The second call costs
  more than the first, and the third and fourth no more. With r300-3 (6.9, 7.2, 7.3 and 6.4), the link looks per
  call rather than shared bandwidth beyond the first call. It stays tagged [inference] in the simulator: neither run
  was built to isolate it.
- **Commit p99 with only the far voter is the far round.** After the sever, each commit waits for apac, about 220 ms
  a round at p50, so commit p99 of 0.9 to 1.6 s is the gate's ordinary queueing on a far quorum. B0-02's line
  assumes the near voter.

## B0-02 gate on this tree

Site day 21:48 to 21:56Z on `78f0749` (`SHARD_INSTANCE=standard-4`, `--test-faults`); runs 22:05 to 22:33Z, each on
a new group, `arrival=mem`, rates alternating run by run (`PREFIX=m0c9g GATE=1 bash tests/m0carried/ab7.sh`). `6f9f697`
adds no code to `78f0749`. 0 failed commands in all 16 runs, no leader change, no STEP_DOWN, no alarm. The leader was on
site 0 in every run. Raw rows: `tests/m0/results/raw-carried-9/m0c9g-*`; the per-run report is `m0c9g.gate8.jsonl`
(`python3 tests/m0carried/gate8.py m0c9g`).

| Run | Throughput (commands/s) | Commit p99 (ms) | Leader confirm p99 (ms) | Commands per entry | CPU p999 (ms) | 80 ms+ `/commit` turns | Verdict |
| --- | --- | --- | --- | --- | --- | --- | --- |
| r300-1 | 280 | 467 | 43 | 48.9 | 22.8 | 1 | Pass |
| r300-2 | 280 | 543 | 53 | 52.3 | 26.2 | 0 | Pass |
| r300-3 | 280 | 374 | 46 | 39.4 | 27.1 | 1 | Pass |
| r300-4 | 279 | 253 | 44 | 34 | 25.5 | 1 | Pass |
| r300-5 | 279 | 424 | 89 | 43.4 | 35.8 | 6 | Pass |
| r300-6 | 280 | 280 | 57 | 38.4 | 26.8 | 1 | Pass |
| r300-7 | 280 | 268 | 56 | 34 | 18.0 | 0 | Pass |
| r300-8 | 277 | 420 | 109 | 43.1 | 30.7 | 3 | Pass |
| r330-1 | 306 | 510 | 71 | 53.5 | 26.6 | 0 | Pass |
| r330-2 | 307 | 403 | 60 | 46 | 29.3 | 1 | Pass |
| r330-3 | 307 | 530 | 78 | 56.9 | 34.3 | 3 | Pass |
| r330-4 | 308 | 444 | 93 | 48.2 | 24.1 | 3 | Pass |
| r330-5 | 308 | 490 | 64 | 54 | 26.9 | 2 | Pass |
| r330-6 | 308 | 526 | 179 | 53.2 | 44.6 | 4 | Pass |
| r330-7 | 308 | 379 | 107 | 50.5 | 30.6 | 4 | Pass |
| r330-8 | 308 | 518 | 72 | 55.5 | 26.2 | 0 | Pass |

**16 of 16 pass** [measured]: 8 of 8 at 300 offered (279 to 280 commands/s, commit p99 253 to 543 ms) and 8 of 8 at 330
(306 to 308 commands/s, 379 to 530 ms). The line is at least 300 commands/s at commit p99 at most 600 ms; the 300
offered runs measure 280 because the bench paces in 1 s windows, as in every gate since `carried-6`, and the 330 runs
carry the throughput clause.

The operator's three conditions from `carried-8`, checked again on this tree:
- **Turn CPU.** CPU p999 18.0 to 44.6 ms, median 26.9, against 23 to 49 ms, median 30, on `carried-8`'s gate. `/commit`
  turns of 80 ms or more: 30 over 16 runs (0 to 6 a run), against 28 on `carried-8` and 24 on the 60 KB tree. Commands
  per entry 34 to 57, below `carried-7`'s uncapped 85 with its 11 heavy turns in 20 s. The heavy turns did not come
  back, and no run's commit p99 tracks its heavy-turn count (r300-5 has the most, 6, at 424 ms).
- **Confirm p99 by entry size, all 16 runs** (n, p99 ms): 0 to 40 KB 1,628, 56; 40 to 60 KB 3,880, 84; 60 to 80 KB
  890, 100; 80 to 100 KB 169, 156; 100 to 130 KB 45, 125. `carried-8` had 108 ms at 80 to 100 KB and 131 ms at 100 to
  130 KB. The 80 to 100 KB bin's 156 ms is mostly r330-6 and r300-5, the two runs whose confirm was slow at every size
  (r330-6: 99 to 270 ms across bins), so it is the storage, not the size [inference].
- **A call carries one whole entry.** `a_voters_call_bound_halves_on_lost_calls_and_grows_back_on_answers` sends a
  300 KB entry with the bound at its floor and asserts the call carries it.

**The far voter in the gate.** Calls to apac missed `CALL_DEADLINE` in 3 runs: 14 in r300-7 (latest answer 1,559 ms)
and 1 each in r330-3 and r330-5 (1,027 ms). Each answered late, inside `LATE_LIMIT`, so none counted as lost and none
stopped the pipeline or halved apac's bound. On `6d2ab81` the same misses counted against `link_lossy` and turned
pipelining off. r300-7, with the most misses, had the second-lowest commit p99 (268 ms), because the near voter makes
the quorum. The leader stayed on site 0 in all 16, so the gate did not exercise a weur leader; `m0c9-far2` above and
the simulator test cover that.

Against `carried-8`'s gate on `6d2ab81` (16 of 16, 249 to 579 ms), this tree's range is 253 to 543 ms. The two gates
ran on different site days, and the difference is within run-to-run spread.

## Cost

`graft-m3d` from 20:40Z to 23:10Z cost $3.67 (`INSTALL=graft-m3d python3 tests/fanout/m2bcost.py
2026-11-13T20:40:00Z 2026-11-13T23:10:00Z`): one aborted site day start (no deploy), one site day, the two hosted
far-link runs and the 16-run gate. With `carried-8`'s $28, this thread's B0-02 work is about $32 of the $55 cap.
