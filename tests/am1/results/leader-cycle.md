# B0-33's leader cycle: why agent throughput stalls near 82/s against 140 (2026-11-21, $0)

The orchestrator (T-01a17bda) asked where each cycle's time goes on B0-33 m64r33 (`tests/am1/results/b033-m64.md`):
the round, the wait for deps, decide and apply, and the gap that one entry in flight leaves. It also asked for the
largest gap and the smallest change that would close it, as a proposal only, and what that change does for B0-29's
queue tail. Nothing here is built. Tags follow `design/SHARED.md`, "Read this first".

**Sources:** `tests/fanout/raw/tsave-m64r33.jsonl.xz` and `raw/a1-pushes-b0-33-mwpnhiva.jsonl` (both on branch
`b033-m64`). B0-29's trace is `tests/fanout/raw/tsave-m3fb29.jsonl.xz` on main.

**Method:** `tests/fanout/leadcycle.py`, new on this branch, run as:

```
python3 tests/fanout/leadcycle.py tsave-m64r33.jsonl.xz 0:2026-11-21T00:12:01Z:2026-11-21T00:14:13Z \
  1:2026-11-21T00:14:14Z:2026-11-21T00:17:01Z 0:2026-11-21T00:08:00Z:2026-11-21T00:11:50Z
```

Leaderships: site 0 led 00:12:01–00:14:13 and site 1 led 00:14:14–00:17:01. The third span is site 0 idle, before
load.

## Where a cycle goes

A cycle runs from entry k's first AppendEntries to entry k+1's, on the leader's clock. "Full" means entry k was cut
`AgentCmds` or `AgentTokens`, so a backlog was waiting.

| Leader | Kind | n | Cycle | Round (to first voter ack) | Ack → own Applied | Applied → cut | Tail: deps overrun, `decide`, encode | Batch frozen before commit |
|---|---|---|---|---|---|---|---|---|
| site 1 | full | 210 | **509 ms** | **412 (80%)** | 0 | 0 | 97 (19%), p90 273 | 210 of 210 |
| site 1 | other | 151 | 387 | 330 (85%) | 0 | 0 | 57 (14%) | 151 of 151 |
| site 0 | full | 89 | **858** | 525 (61%) | 96 (11%) | 0 | 236 (27%), p90 900 | 89 of 89 |
| site 0 | other | 136 | 338 | 226 (67%) | 3 | 36 (10%) | 72 (21%) | 128 of 136 |

- **Round.** On site 1, the clean leader, the round is 80% of a full cycle. The deps read overlaps it: every full
  batch was frozen and its read asked before k committed.
- **Wait for deps, `decide` and apply.** These sit in the tail. Site 1's tail is p50 0 and mean 97 ms. Its own apply
  was never behind the follower's ack (0 ms).
- **The one-in-flight gap** (apply + wait to cut + tail): 97 ms on site 1 (19%), 332 ms on site 0 (39%).
- **Site 0 was slower for its own reasons.**
  - 12 storage confirms of 1 s or more, against 3 for site 1 and 0 for site 2. `tests/fanout/syncs.py` labels 9 of
    them `storage` and 3 `isolate late`. As a follower after 00:14:14, site 0's confirms were p99 144 ms.
  - Ticker lag at its sends: p90 403 ms, p99 1,575 ms. Site 1's was 0 and 0.
  - 16 slow alarms on site a summing 28.1 s. The largest steps were split 6.7 s, load 4.8 s and merge_queue 4.1 s.
  - Its two 200 ms confirms at 00:14:09.5 met `LOOP_LAG`, which is the likely trip behind its one leader change
    (`tests/am1/results/transfer-why.md`).
- **Ceiling arithmetic.** Site 1's full cycles carried 64.4 commands per 509 ms, about **126 commands/s** while
  backlogged. Site 0's carried 64 per 858 ms, about **75/s**. A cycle of one idle round (about 100 ms to the near
  follower) plus a 97 ms tail would carry about 320/s.

### What the round is made of under load

Each voter call's leg (`CallSent` to `AnswerArrived`, leader clock) less the follower's handler (`CallLeft`, follower
clock):

| Leader → follower | Calls | Leg p50 / p90 | Follower handler p50 | Outside the handler p50 |
|---|---|---|---|---|
| site 0 → 1, idle (00:08–00:11:50) | 210 | 94 / 113 | 0 | 94 |
| site 0 → 2, idle | 210 | 185 / 200 | 0 | 185 |
| site 0 → 1, loaded | 276 | 233 / 974 | 13 | 203 |
| site 1 → 0, loaded | 384 | **322 / 614** | 9 | **299** |
| site 1 → 2, loaded | 393 | 392 / 751 | 39 | 336 |

- **The followers are not the cost.**
  - Handler p50 is 9 to 39 ms, storage confirms p50 15 to 34 ms, and ticker lag 0.
  - Followers entered 10 to 16 requests/s with p50 1 open at entry.
- **The loaded round is about 3× the idle leg** (299 against 94 ms to the near follower), and the excess is outside
  every handler. That leaves wire, input queues, the follower's output gate and the leader's delivery of the answer.
  Site 1 took 250 to 450 requests/s with p50 262 open at entry, and its loop lag was 0.
  - This trace cannot split that excess further, because the two sides use different clocks.
  - `tests/fanout/applegs.py` (Workers Logs invocation starts, `tests/am1/results/round-time.md`) can split it, and
    it is the next $0 measurement.
- **Part of the excess is entry bytes** (site 1's first-ack round by AppendEntries body):

  | Body | Full: n, round mean | Other: n, round mean |
  |---|---|---|
  | under 40 KB | 104, 321 ms | 61, 210 ms |
  | 40 to 65 KB | 28, 314 | 7, 225 |
  | 100 to 150 KB | 13, 367 | 12, 339 |
  | over 150 KB | 59, **440** | 58, **429** |

  - Over 150 KB costs about 120 ms per round on full entries and about 220 ms on others. The 64 KiB body step is in
    `tests/m0/results/carried-6.md` and `carried-7.md`.
  - Of m64's 725 non-empty cuts, 373 were over 64 KiB. 77% of all cut bytes were outside `ENTRY_SOFT`'s count, which
    is mostly Agent inline packs, bounded by `AGENT_INLINE_ENTRY` (128 KiB) [measured, cut rows' `bytes` against
    `soft`].
  - 16 cuts over 300 KB had p50 777 KB outside `soft` with 64 Agent commands, more than `AGENT_INLINE_ENTRY` should
    allow. Why is [unverified]: worth a check before any change rests on that bound.

### Why the backlog was not there a third of the time

- 285 of 595 entries were Empty-ended (22.5 commands each, 352 ms cycles), about a third of leader time.
- In those windows the agent pushes had not reached the leader:
  - From 00:14:30 on, the edge's `validate` stage took p50 0.4 to 16.4 s and p90 13 to 77 s, against p50 0.27 s
    before.
  - The container's own split explains none of it: `v_lock_wait` 0, `v_index` at most 86 ms, `v_walk` at most 110 ms.
  - 561 of 1,172 pushes started in 00:14:30–00:15:00 and 644 of 1,170 in 00:16:30–00:17:00 spent over 5 s there.
- Those pushes reached the leader past their 5 s `AGENT_PUSH_BOUND`. That is the row's 2,009 `bound_at_send` and part of
  its 2,360 `admission` refusals.
- Where `validate`'s time went (the shard fetch, a slot, or the container's queue) is [unverified]. The edge records
  only the container's three stages.
- During backlog, the queue held p99 806 Agent commands after a cut. At about 126/s that is a 6 s wait, past the 5 s
  bound, which accounts for `expire_agents` 2,139.

## The largest gap, and the smallest change

**Largest gap: the round on full entries**, 412 ms of a 509 ms cycle on the clean leader, against an idle leg of about
100 ms. The one-in-flight gap (97 ms) is second.

The round's loaded excess has two measured parts:
- entry bytes, about 120 ms between full entries under 40 KB and over 150 KB;
- an unattributed remainder outside the followers' handlers.

Separately, upstream `validate` starvation took about a third of leader time out of backlog.

**Superseded by `tests/am1/results/inline-cut.md`:** the bound below was scaled in raw bytes against base64 entry bytes, so at 32 KiB 150 m3fb29 entries stay over 64 KiB, not 15, and 59% of journey 9's agent pushes would stage. The proposal as first written:

**Proposed (not built): lower `AGENT_INLINE_ENTRY` from 128 KiB to 32 KiB,** so an entry's Agent inline packs keep its
AppendEntries body under the 64 KiB step. An Agent pack that would pass the bound is staged (`PackRef::Stored`) as
today. It is a one-constant change in `crates/raft/src/lib.rs`, plus its design line and a DECISIONS entry with a flip.

- **B0-33, expected [inference]:**
  - Full-entry rounds on site 1 move toward the under-40 KB mean (321 ms): full cycles 509 → about 430 ms, about
    150 commands/s while backlogged, against 126.
  - Over the row that is about +10 to 15% admitted, about 90 to 95/s.
  - **That alone does not reach 140.** It needs the unattributed round excess and the `validate` starvation as well.
- **B0-29, expected [inference]:**
  - On m3fb29, 529 of 3,384 cuts were over 64 KiB. Cuts over 60 KB averaged 88 KB with 8 KB under `soft` and about 5
    Agent commands, so about 80 KB of Agent packs. With a 32 KiB bound, 15 would remain over 64 KiB.
  - The m3fb29 split (thread T-01a1d805) found every tail command waiting on frozen batches behind a large entry in
    flight: 62 against 31 KB, append to apply 295 / 603 ms against 179 / 352 ms for all entries.
  - Bounding the bytes shortens that entry in flight, so the p99 wait per batch ahead moves toward 352 ms. Commit p99
    could move from the 1,102 ms median toward about 850 to 900 ms.
  - So one change can be tested on both gates.
- **Cost, to check on the row:**
  - Pushes between 32 and 128 KiB now stage. A36 measured the `Stage` round trip at p50 546 ms and the R2 place at p50
    638 ms per staged push.
  - That lands on those pushes' end-to-end time, and on B0-32's within-5 s line, not on a leader's commit stage.
  - While placement fails, those pushes get the Agent refusal at once.
- **Flip:** a B0-29 or B0-33 row where full-entry rounds over and under 64 KiB differ by less than 50 ms (bytes are not
  the cost), or where staged pushes take B0-32's within-5 s line below its m3fpc1 value.
- **Not proposed:**
  - Pipelining entries breaks the one-entry-in-flight invariant (AGENTS.md).
  - Raising `AGENT_CMDS_ENTRY` meets its B0-32 flip, and on this trace count and load are confounded.
  - Shortening rounds directly is F106's.
