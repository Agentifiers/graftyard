# read-beat: B0-29's commit regression traced, and a read round that re-sends nothing

The operator asked for a code-only trace of B0-29's regression: commit p99 went from 774 ms on m27 (run `mwga2kd3`, 2026-11-14) to 1,682 ms on `mwkyk9zg` (graft-m2b, main `19b5ae77`, 2026-11-17; `tests/am1/results/b029-m2b.md`). There were no hosted runs. The sources are `tests/fanout/raw/tsave-m2bb29b.jsonl.xz` (load window 1794936481594 + 600 s, leader site 0), the a1 rows `tests/am1/raw/a1-b0-29-mwkyk9zg.jsonl` and `a1-b0-29-mwga2kd3.jsonl`, and m27's member legs.

**Baseline correction.** m27 ran on main `2b1938e` (`tests/am1/results/lm1-memory.md`, round 16), not `77c9fa23`. The window to diff is `2b1938e..19b5ae77`.

## Where the time goes

| Leader split | m27 `mwga2kd3` | `mwkyk9zg` |
| --- | --- | --- |
| commit p50 / p99 | 360 / 774 | 513 / 1,682 |
| entries for about 10,000 commands | 2,895 (4.8/s) | 2,268 (3.8/s) |
| `queue:slot` p50 / p99 | 158 / 462 | 903 / 4,308 |
| `round` p50 / p99 | 157 / 336 | 202 / 514 |
| leader `confirm` p50 / p99 | 21 / 58 | 51 / 231 |
| `append:1` / `append:2` p99 | 361 / 457 | 456 / 704 |
| `append:1` + `append:2` calls | 5,796 (one per entry) | 31,792 (6.3 and 7.7 per entry) |
| inbound `/raft` on the leader | 54,395 | 93,933 |

Commit time is the wait for the group's one entry in flight (`queue:slot`), and that wait grew because the leader cut fewer entries a second. Each entry's round, the leader's own confirm and both follower legs got slower [measured]. The rest of this section is what slowed them.

**Every read round re-sent the entry in flight to both voters.**
- `AppendQueued` (trace kind 12, logged only for calls that carry entries) shows each entry index sent on average 6.35 times to site 1 and 7.81 times to site 2.
- Repeats were 11,941 of 14,173 sends to site 1 (84.3%) and 15,146 of 17,370 to site 2 (87.2%). They carried 639 MB and 936 MB, against 111 MB and 113 MB of first sends.
- 11,923 and 15,130 of those repeats left while an earlier send of the same index was still unanswered. All but a few carried the same bytes as the first send [measured].
- The followers took 29 to 31 calls/s at 1.25 to 1.75 MB/s. m27's took 2.8 to 3.0 calls/s at 0.18 to 0.21 MB/s, at about the same bytes per call [measured, m27 `raftIn`].

**The repeats are read rounds, from the code.**
- A send without force either pipelines the entries after the last in flight or returns, so it never repeats a last index.
- The only forced send that can fire about 25 times a second is `read_round` → `send_append(p, true)`. It drops the pipeline (`pipe.remove`) and sends again from `next`, which moves only on an ack. So each round carries the unacked entry again.
- The cadence matches: the gap between sends of one index is p10 18 to 20 ms and p50 28 to 29 ms, which is READ_ROUND_GAP or an answered round. 12,104 (ms, index) pairs went to both voters in the same millisecond.
- 63 to 70% of repeats share their millisecond with an `IndexTaken`, a learner's read-index arriving.
- `tick_leader` resends only after CALL_DEADLINE, and `append_failed` covers at most the 133 failed calls.

**Why every read needs a round: lease-clock (`27eece68`, 2026-11-15).**
- Since lease-clock, `start_read` grants no lease read while `clock_suspect`, the largest of the last 8 stamp lags being 250 ms or more.
- On the leader, the lag of each arriving `/raft` call (sender stamp minus leader clock) was p50 216, p90 540 and p99 3,083 ms. The 8-call window was at or above 250 ms for 86.7% of arrivals.
- Of 27,361 matched read-index grants, 23,590 arrived with that proxy suspect, and only 6,314 of those were granted in the same millisecond.
- On the followers the same lag was 0 (site 1) and at most 122 ms (site 2) [measured].
- The run saved no `/stats.clock`, so the flag itself was not observed, only this replay of its rule [inference].
- At `2b1938e`, `start_read` forced a round only when the lease had lapsed, which a leader with live followers rarely does.

**The commits between.**
- read-rounds (`c6b3d0a8`), read-round-gap (`0f1c3c90`) and fanout-dups (`2a7aca8c`) cap rounds at one per voter per answer or per 20 ms, and one per turn. They did not change what a round carries.
- None of the operator's five candidates, query-tag `19937a9f`, probe-known `e39df0c7`, burst-receipts `3facf546`, lookup-poll `828557d6` and push-reads `cb07b332`, touch `read_round` or `append_to`.
- lookup-poll, probe-known and query-tag change edge lookups, which this run, with term 2 throughout, barely exercised.

## deps:tokens and deps:names p99 at exactly 500 ms

No timeout bounds the dependency read. `Host::read_deps` calls `lookup::call` (CALL 3.5 s, LOOKUP_FOR 20 s), and a leader's plain `/read` waits up to three CALL_DEADLINEs. The 500 is the leader isolate's clock, snapped to `READ_INDEX_HOLD` timers.
- A learner's `ReadIndex` call to the leader stays open up to READ_INDEX_HOLD (500 ms, `workers/site/src/member.rs`, `raft_steps`) for its answer. 27,723 of the 92,092 learner calls the leader answered left exactly 500 ms after entry (`CallLeft` minus its entry stamp). Voter calls left at p99 10 ms.
- A Workers isolate's clock is the time of the last event it polled. When a hold timer fires, the work handled in that turn reads the timer's time.
- On the leader, 17,179 of the 34,474 milliseconds that had an `IndexTaken` also had a row exactly 500 ms later. Every other offset from 480 to 520 ms had about 4,700 to 4,900. Across all leader rows, 108,565 of 635,904 (17%) sit on such an instant.
- The stamp lag of calls arriving at those instants is p50 312 ms, against 202 ms at other times, so these are busy turns that read a timer's time [measured].
- So a dependency read that started in a learner call's turn and finished in the turn of that call's hold timer measures exactly 500. Past 1% of reads doing that sets p99 at 500 [inference from the trace; the dependency reads themselves are not traced].
- No earlier run's `deps:tokens` or `deps:names` sits at 500 (every a1 row under `tests/` checked). The holds are as old as B0-18 (`06762a26`, 2026-11-07), so what is new is how often the timers fire on a busy leader.

**Second finding: most of those holds wait for nothing.**
- The hold loop waits until its batch's `ReadIndexRes` is in its own response, or held for the caller. A learner keeps up to READ_PIPELINE batches in flight, though, so its calls overlap; they are mostly 3 to 5 open at once.
- A concurrent call's response takes everything held (`Callers::take`). In 26,877 of the 27,662 calls held the whole 500 ms, the batch's answer had already left in another call of the same learner, so the hold could never end early [measured].
- Each such learner call is answered 500 ms late. With HEDGE also 500 ms, the learner resends the call: 18,619 hedges, all answered. Each hedge is another `ReadIndex` the leader starts a read for, so roughly 16,000 `IndexTaken` rows have no `ResSent` [measured counts; that they are the hedges is an inference].
- Fixing it is in `workers/site/src/member.rs`, not `crates/raft`: end the hold once the batch's answer was taken by any call. It is recorded here as the next candidate and is not on this branch, because it predates m27 and the regression does not need it.

## `/outcome` 565 against 7, and `/raft` 94k against 54k

- **`/outcome`:** the edge posts `/outcome` from `commit_hedged` (`crates/lookup/src/lib.rs`) only when `/commit` answered `admitted`, which means held past COMMIT_HOLD (1 s). The trace has 558 `/outcome` from edge isolates (SentBy Other). 565 of 10,931 commits is 5.2%, consistent with p99 1,682 ms. It is the slow commits counted again, not a cause. `colo-unknown` (`fbe49d59`) changes `/suspect` counting, not this.
- **`/raft`:** 92,092 of the leader's inbound calls came from learners and 847 from voters (`CallLeft`). Of the learner calls, 18,619 were hedges and about 73,500 were read-index batch calls.
  - The learners' rate sits near their ceiling: 8 learners, at most one batch per 50 ms each, is 96,000 in 600 s.
  - Each batch now waits a round instead of a lease grant (grant p50 36 ms, p90 131 ms), so learners keep more batches in flight.
  - m27 saved no per-sender split, so its learner share is not known.

## `GRAFTYARD_ROLLUP=1`: ruled out

It gates only `log_archive::roll` (`crates/git/src/log_archive.rs`), reached from `workers/site/src/archive.rs` `rollup()`. It runs only in an idle archive alarm step, after archive ranges and promotions. `roll_plan` needs 2 or more stored promotions and either 2 GiB of them or 8 packs with the oldest 24 h old. A repository created for a 10-minute row has none, so the step returns `Idle` before any write, and nothing on the commit path reads the variable [read from the code].

## The fix: a read round to a pipelined voter carries no entries

`Member::read_beat` (`crates/raft/src/member/leader.rs`) applies to a voter with AppendEntries out (a non-empty pipeline and an outstanding `sent` that left under READ_RESEND, 500 ms, ago) whose acknowledged index is in the in-memory window. For that voter, a read round sends an AppendEntries with no entries at `prev = matched`, the voter's acknowledged index, and leaves `pipe` and `sent` alone.
- **Acceptance.** The voter holds `matched` with that digest, so it accepts the round whatever its calls in flight carry. Its ack counts in `acked_at` and renews its promise like any other.
- **Read safety.** Grants still need a voter quorum's ack of a round sent at or after the read arrived. The round's `sent_at` is the leader's now, and `check_reads` is unchanged, so read safety is the same argument as before.
- **No re-send.** The answer starts nothing: `next` is already past the entry in flight, and the non-forced send after an ack returns while the pipeline covers `last`.
- **Fallback.** Any other voter gets the round exactly as before, a forced send from `next`: none out, a lossy link (no pipeline), a catch-up, an install, an acknowledged index below `base`, or an outstanding call READ_RESEND old or older.
- **Failures.** A failed empty round names a round that is neither `sent` nor piped, so `append_failed` ignores it. It still counts in the link's health, as a forced round's failure did.

What it beat:
- **Restoring lease reads while the clock is suspect:** that reopens the stale-read window lease-clock closed (`tests/am1/results/lease-clock.md`).
- **Fewer rounds (a longer READ_ROUND_GAP):** each read waits longer, and every round still re-sends the entry.
- **A round only to voters with nothing out:** a read arriving while an entry is in flight would then wait for that entry's ack, coupling read latency to commit latency.
- **Empty rounds with no resend (this branch's first version):** failed the long sweep's live bound, below.

Expected effect [inference]: removing most of the leader's repeat AppendEntries to voters (84 to 87% of its calls to them, 640 to 940 MB a run) should bring the follower legs and the leader's confirm back toward m27's, and with them the entries cut per second and commit p99. Reads still take a round while the clock is suspect, now an empty one. Whether the clock stays suspect once the leader is less loaded is for the hosted rerun to show.

## The long sweep

The first version had no READ_RESEND: an empty round went to any voter with a call out. Its long sweep (`RAFT_SEEDS=2000 RAFT_STEPS=10000`, 1,190 s) passed the harsh half (longest stall 14,979 ms, seed 11,636) and the gray half (24,376 ms, seed 10,183), and failed the live half: seed 13441 stalled 31,166 ms from 125,318 ms, past the 30 s bound. Porcupine was Ok on the harsh history (144,172 operations) and the gray one (2,235,105).
- **On main the same seed stalls 5,273 ms.** Replayed with the empty rounds turned off from a chosen time, the seed stalls 10,398 ms when they stop at 120 s and 1,835 ms when they stop at 100 s: the divergence starts before the stall, as early as 100 to 110 s.
- **The stall.** Leader site 0 in term 11, with site 1 half-hung (its answers lost), lost its calls to site 1 and kept sending empty rounds to site 2. Nothing re-sent the lost calls until CALL_DEADLINE's tick, and site 0 handed off to site 2 at 126.9 s. Site 2 won term 12 and lost it at once to site 0's pre-vote in term 13. Site 0 was then refused as `LogBehind` and site 1 as `Promised`, and no leader served until site 1 won term 14 at 146.3 s and committed at 156.5 s.
- **The forced rounds hid a recovery path.** Each one re-sent a lost call within READ_ROUND_GAP.
- **Distribution.** Over 12,000 live seeds (10,000 to 21,999) run per seed without the bound, main and the first version match: p50 4,372 against 4,544 ms, p99 16,005 against 15,998, p99.9 21,554 against 22,395, and 30 against 29 seeds over 20 s. The one seed over 30 s is 13441. So the regression is one path, not a shift of the whole tail.

READ_RESEND restores that path for a call outstanding 500 ms, half of CALL_DEADLINE, as the learners' HEDGE. Seed 13441 then stalls 3,025 ms (100 ms gives 1,803 and 250 ms gives 7,736; 1,000 ms keeps the 31,166 failure). On `mwkyk9zg` a voter's first ack of an entry came p50 206 / 319 ms, p99 504 / 739 ms after its first send, so 25 of 2,230 and 91 of 2,222 entries would get one more send; main sent 5 to 7 more of every entry.

## Results

- **Regression test:** `a_read_round_behind_an_entry_in_flight_resends_no_entries` fails on main `fbe49d59`: "20 of 20 read rounds re-sent the entry in flight". It passes on the branch.
  - It holds an entry's calls unanswered, starts 10 reads 25 ms apart on a suspect clock, and checks three things: no round carries the entry, every read is granted by the acks of the empty rounds alone, and the held calls then commit the entry on both voters.
- **Fallback tests:**
  - `a_read_round_to_a_voter_with_no_calls_out_still_carries_the_entry` covers a voter whose calls all failed.
  - `a_read_round_resends_a_call_outstanding_past_read_resend` covers lost calls: an empty round inside READ_RESEND, the entries re-sent past it, before CALL_DEADLINE.
- **The `graftyard-raft` crate passes.** That includes `reads_send_at_most_one_round_per_gap_while_the_clock_climbs_and_no_learner_gets_it` and `reads_send_at_most_one_round_per_voter_per_turn_while_the_floor_jumps`.
- **Plants:** `tests/m0carried/plant33.py`, 6 of 6 caught:
  - the round still re-sends from `next`;
  - the empty round at the last index rather than the acknowledged one;
  - an empty round to a voter with nothing out;
  - no READ_RESEND;
  - the empty round resetting the pipeline;
  - an old `sent_at` on the empty round.
- **Long sweep** (`RAFT_SEEDS=2000 RAFT_STEPS=10000`, 1,182 s): passed. Longest healthy-quorum stall: harsh 14,776 ms (seed 11,372), gray 25,170 ms (seed 11,459), live 25,413 ms (seed 11,267, main's own worst over the same seeds).
- **Porcupine** (`tests/m0/porcupine`, `-seg 2000`): h Ok, 146,279 operations; h.gray Ok, 2,239,019; h.live Ok, 5,955,650.
- **Full checks** on the branch: fmt; workspace clippy; wasm32 clippy for site, edge, and ops (site and edge with and without `test-faults`); `cargo test --workspace` 1,116 passed and 0 failed; `go test` in `containers/git/server` ok; and the three `scripts/check_*.py` OK.

## Not done here

- No hosted run. The operator schedules the B0-29 rerun on a test-faults install after a site day from this branch. Things to read in it:
  - `append:1` and `append:2` n near one per entry;
  - follower `/raft` near m27's 3 a second;
  - `queue:slot` and commit p99;
  - the leader's `/stats.clock`, which tells whether it is still suspect.
- F83 (the learner hold) is open and not on this branch.
