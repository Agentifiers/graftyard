# B0-32: lever B (a lower `AGENT_INLINE_ENTRY`) and the line question (design, not built, $0, 2026-11-23)

Asked by the orchestrator after LM-1's `seal-bytes-design.md` (main `dddfbca7`). Two parts, both at $0:
1. lever B's design, with its missing number measured from committed raws, priced as two branches that LM-1's probe (c)
   picks between;
2. the line question for B0-32 if (c) reads per transaction, in numbers only.

The only reads were committed raws and a read-only `hostguard.py` replay over Workers Logs. Nothing was built and nothing ran on an install. Tags follow `design/SHARED.md`, "Read this first".

**Short answer.**
- **Lever B can reach ×0.63 on m3fsf2 in one case only:** cost per byte, a cap near 12 KiB, and the leader's `/raft` CPU taken as the whole limit. With a 16 to 24 KiB cap the line prices at about 393 to 436 on that reading, and 299 to 315 counting all the leader's routes. If the cost is per transaction, it prices at 284 to 292 [inference, below].
- **The pushes that pay for lever B aren't on B0-32's line.** Burst pushes are 256 KiB and already stage. A lower cap stages journey 9's background agent pushes. On B0-29's raws a staged background push costs about +320 ms at the edge, p50 [inference from measured parts, below]. That isn't +170.
- **B0-29's commit line shouldn't move. B0-33's agent line could** [inference]. Lever B also reverses a recorded decision whose stated reason used a superseded B0-29 line (below).
- **The line question:** on kept hosts the best B0-32 runs read 436 to 496 at 5 s and 500 at about 6 s. Those hosts were 4 of the 16 graft-m3f leaders measured.

## 1. Lever B: `AGENT_INLINE_ENTRY` 128 KiB → 16 to 24 KiB

### What the bytes are

Every B0-32 burst push is `randomBytes(256 << 10)` (`tests/am1/rest/a1.mjs`, `planFor`). That is over `AGENT_INLINE_ENTRY`, so it stages first: 485 of 500 carry `stagefirst` on m3fsf2 and 496 of 500 on m3fg3 [measured, the a1 files' `burst timing`]. The agent inline bytes in the burst's entries come from journey 9's background agent pushes, 10,000 to 70,000 B uniform (`tests/am1/rest/wire.mjs`, `journey9`). The leader packs those up to the cap.

**Entry bytes over the burst's first 6 s** [measured, `tsave` `AppendQueued` (kind 12), each entry's smallest body; cut rows (kind 29) via `tests/m0/cuttrace/cuts.py`]:

| | m3fsf1 (27.3 ms) | m3fsf2 (52.5) | m3fg3 (55.8) |
|---|---|---|---|
| Entries; AppendEntries body mean, p50, max | 24; 42, 40, 170 KB | 18; 107, 121, 238 KB | 13; 132, 136, 230 KB |
| Agent inline share of body (cut inline × 4/3, base64) | 41% | 74% | 88% |
| Body bytes removed at a 24 / 16 / 8 KiB cap | 19 / 25 / 32% | **51 / 58 / 66%** | **63 / 71 / 80%** |

- A cap clips each entry's agent inline bytes; packs past it stage as `Stored`, and their small command stays. The table counts only the clip, so the real cut is a little less.
- The 62% that `seal-bytes-design.md` derives for ×0.63 needs a cap of about 12 KiB on m3fsf2, between the 16 and 8 KiB columns. On m3fg3 it needs about 24 KiB. My earlier note to LM-1 measured cut `bytes` (a 50 KiB cap gave 32% and 38%), which this body measure supersedes: `inline-cut.md` found that cut `bytes` can carry reply renders.

### The missing number: background agent pushes, staged against inline [measured]

B0-29's five m2b rows have the edge's push lines committed (`tests/fanout/raw/edgepush-m2bb29{h,i,j,k,n}.jsonl.xz`). Each line joins by `client` to a1's push log, which gives kind and result (`tests/am1/raw/a1-pushes-b0-29-*.jsonl`, on this orb). The figures below are `ok` agent pushes, each push's own `server-timing`, in ms.

| Agent `ok` push | n | Edge total p50 / p90 / p99 | `validate` p50 | `stage` (place) p50 | Commit stage p50 (all commits) |
|---|---|---|---|---|---|
| Inline, one commit | 8,931 | 1,745 / 2,579 / 4,092 | 249 | — | 612 |
| Staged first (1 MiB packs, over the cap) | 67 | 2,076 / 2,466 / 5,344 | 570 | — | 552 |
| Inline refused `Stage`, then staged (10 to 70 KB packs: the entry's budget was full) | 1,045 | 2,491 / 3,533 / 5,020 | 254 | 563 to 594 by row | 743 |

- **Per row, the inline-then-refused pushes cost +661 to +862 ms at p50 over inline.** For example, m2bb29h: 2,271 against 1,586, and m2bb29i: 3,024 against 2,162. Those pushes are the size class lever B would stage, and today they pay the refused commit, the place and a second commit.
- **Staged first, the same pushes would pay about +320 ms at p50** [inference]. The edge places a staged-first pack beside validate (`place_teed`), so the push waits max(place, validate) instead of validate: about 570 ms against 250. The staged-first 1 MiB pushes show that bound directly, with `validate` p50 570 against 249 inline and an edge total of +331 ms. Their packs are larger, though, so that row is an upper bound for small packs.
- **The +170 in my earlier note was B0-32's figure** (m3fhs1, validate staged first 527 against 361 inline). On B0-29's hosts, where validate is faster, the place sets the wait.

### Who pays, and whether the lines move [inference]

At a 24 KiB cap, about 72% of journey 9's agent pushes newly stage. At 16 KiB, about 85% do (the 95% of the draw that is uniform 10,000 to 70,000 B, over the cap). Each pays about +320 ms at p50, and the place's two bucket writes, which `scripts/cost.py` already prices per push.

| Row | Newly staged | What a staged push changes | Line |
|---|---|---|---|
| B0-32 | about 3 of 4.2 background pushes/s; 0 of the 500 burst pushes | Background pushes aren't on the line. They add about 3 places a second beside the burst's 500 in 1 s | **Unchanged by the cost.** Only the byte cut (section "Price") moves it |
| B0-29 | about 3.0 (24 KiB) to 3.5 (16 KiB) of 4.2 agent pushes/s | Push end-to-end +320 ms at p50. The leader's `commitMs`, which the gate reads, excludes staging, and its entries shrink | **Commit p99 (800 ms): not raised by staging, possibly lowered by smaller entries.** Merge p99 (2 s): merges are MergeQueue and never stage |
| B0-33 | about 30 (24 KiB) to 35 (16 KiB) of 41.7 agent pushes/s | +320 ms of each staged push's 5 s `AGENT_PUSH_BOUND` goes before its first send | **Agent throughput could fall.** It is limited by the bound and admission, not leader CPU (`b033-readmemo.md`, 107.3 and 106.8/s against 126). A shorter budget at send means more refused at the bound. Human and merge-queue p99 (3,912 ms) could improve with smaller entries |

**The recorded decision this reverses.** `decisions/DECISIONS.md`'s `AGENT_INLINE_ENTRY` entry, "Approval reversed, 2026-11-21", dropped 48, 32 and 24 KiB. Its reason was that 32 KiB fails the in-memory B0-29 line: commit p99 700 ms against that test's 600 ms line (`inline-cut.md`). B0-29's hosted line has been 800 ms since 2026-11-19 (`FRAME.md`, "Operator decision, 2026-11-19, B0-29's commit line"), and 700 is under it. That model also counts staging inside its commit time, which the hosted `commitMs` doesn't. So the reversal's reason was a superseded line, not the line in force [inference]. The other reason, 72% of pushes newly staged at 24 KiB, stands and is the B0-33 risk above.

### The edit to both agent blocks (proposed, not made)

`AGENT_INLINE_ENTRY` is inside the agent block, so `design/DESIGN.md` (Shape) and `design/AGENT-LAYER.md` (Shape) take the same edit, and `scripts/check_design.py` checks they match:

```
-pub const AGENT_INLINE_ENTRY: usize = 128 << 10;                 // [guess]; and AGENT_CMDS_ENTRY = 64 ...
+pub const AGENT_INLINE_ENTRY: usize = 24 << 10;                  // raw pack bytes, base64 4/3 in the entry; B0-32's slow-host entries were 74 to 88% agent inline (tests/am1/results/b032-leverb.md) [inference until a row]; and AGENT_CMDS_ENTRY = 64 ...
```

- The doc comment above it gets one sentence: on a slow leader host the deferred transaction and apply cost track entry bytes, which the cap bounds, and an agent pack past the cap stages beside validate at about +320 ms at p50.
- The constant's value lands only with probe (c)'s reading cited.
- The code follows the design:
  - `crates/raft/src/admit.rs`, the constant;
  - `workers/edge/src/lib.rs`, `hot_tests`, which uses the constant and needs no number change;
  - `crates/raft/tests/admission.rs`, the in-memory model's expected staged counts;
  - edgeboot (both builds), the raft tests and the long sweep, since `crates/raft` changes.
- Owners to check first: LM-1 for `crates/raft`, and B0-34 for `workers/edge`.

### Price against ×0.63, two branches

m3fsf2, from `heavyturns.py m3fsf2-b0-32`:
- deferred transaction 617 ms;
- apply's byte part 292 ms (LM-1's fit, 0.445 ms a KB × 73 KB × 9 entries);
- joined `/raft` CPU 1,495 ms, and all-routes leader CPU 2,550 ms (Workers Logs, the burst's 6 s).

The line comes from `b032share.py`'s commit scale, x = 1 − removed ÷ CPU, on both denominators. The `/raft`-only denominator is optimistic, because the leader isolate also served `/commit`, `/read`, `/entries` and `/outcome_by_request` in those seconds.

| Branch (probe (c) reads) | Removed at 24 KiB (−51%) | Line | Removed at 16 KiB (−58%) | Line |
|---|---|---|---|---|
| **Per byte:** deferred transaction and apply's byte part scale with bytes | 464 ms: x 0.69 (`/raft`), 0.82 (all) | **about 393** / 299 | 527 ms: x 0.65 / 0.79 | **about 436** / 315 |
| **Per transaction:** the transaction cost stays, apply's byte part scales | 149 ms: x 0.90 / 0.94 | 289 / 284 | 169 ms: x 0.89 / 0.93 | about 291 / 286 |

- **×0.63 (459) needs the per-byte branch, the `/raft` denominator and a cap near 12 KiB (−62%).** No cap reaches it on the all-routes denominator: removing every deferred-transaction and apply-byte millisecond gives x 0.64 there.
- **m3fg3 (55.8 ms) needs ×0.2.** Even removing 71% of its apply and deferred-transaction CPU (446 of 1,034 joined ms) gives x 0.57, and the line stays at 262. That is A44's stall, the host question A41 owns.
- The loss of bunched arrivals isn't modelled: each counterfactual holds arrivals fixed (`b032share.py`).

### Flips

- **Drop lever B for B0-32** if probe (c) puts at most 20% of a 128 KB one-row transaction's hosted cost on its bytes. Then the per-transaction row applies, at a line of about 284 to 292.
- **Revert after a row** in any of these cases:
  - a B0-33 row with the cap admits agent commands under the matched row's rate by more than the kept runs' spread (107.3 and 106.8/s), or under the 100/s fail line;
  - a B0-29 row's commit p99 rises over 800 ms;
  - staged background agent pushes run more than 500 ms over inline at p50.
- **Keep it** if a B0-32 row on a host hostguard reads 45 to 55 ms gets its commit stage p50 under ×0.7 of m3fsf2's 3,757 ms (about 2,630 ms).

## 2. B0-32's line, if no lever reaches ×0.63 (numbers only)

**Every B0-32 leader on graft-m3f with a hostguard reading** [measured, `hostguard.py graft-m3f-site-a repo:N#1#f0` replayed over each row's setup; m3fe2, m3fwi2, m3fwi3 and the m3fpc1-era rows replayed for this file]:

| Row (main) | hostguard | Verdict at 28.4 | Line at 5 s | At 6 s | At 7 s | `ok` edge total p50 / p99 |
|---|---|---|---|---|---|---|
| m3fe2 (`8c794619`) | 23.3 | keep | **496** | 498 (2 unanswered) | 498 | 3,541 / 4,863 |
| m3fwi3 (`b655c323`) | 24.5 | keep | 442 | **500** (max 5,859) | 500 | 4,092 / 5,684 |
| m3fwi2 (`b655c323`) | 24.6 | keep | 436 | about 495 to 500 (p99 5,736; max not in the file) | 500 | 4,034 / 5,736 |
| m3fsf1 (`e942cd53`, front broken) | 27.3 | keep | 323 (about 496 with the front fixed [inference]) | **500** | 500 | 4,439 / 5,787 |
| m3fb32 (`76c3e131`) | 29.8 | re-place | 270 | — | — | — |
| m3fhs1 (`771e07e9`) | 33.6 | re-place | 364 | 463 | 499 | 4,939 / 6,612 |
| m3fac1 (`eaac204b`) | 35.1 | re-place | 311 | 317 | 372 | 7,272 / 8,329 |
| m3fgt1 (`19b4385f`) | 38.5 | re-place | 135 | 161 | 168 | 8,713 / 10,539 |
| m3fpc1 (`a3430143`) | 38.8 | re-place | 251 | 368 | 490 | 5,820 / 7,103 |
| m3fst1 (`f9b9f50e`) | 46.4 | re-place | 332 | 390 | 434 | 5,161 / 7,355 |
| m3fsf2 (`7a1d74c9`) | 52.5 | re-place | 284 | 368 | 492 | 5,823 / 7,212 |
| m3fg3 (`ad8fd52f`) | 55.8 | re-place | 262 | 262 | 262 | 15,742 / 18,542 |
| m3fgb1 (`1a9209c3`) | 71.1 | re-place | 422 (422 `full`) | 427 | 452 | 7,159 / 7,856 |

- The lines at 6 s and 7 s are from each row's committed `burst timing`, counted as `full` plus `ok` within the bound. m3fe2, m3fwi2 and m3fwi3 come from this file's row sections; their a1 files aren't on this orb. m3fb32 is from its section, and the m3fg placements never loaded.
- **Kept hosts:** 4 of the 16 graft-m3f leaders measured, the three m3fg placements included, read under 28.4 (m3fsf1's is EWR; the other three's colos weren't replayed). Every kept-host run reaches **436 to 500 at 5 s, and 495 to 500 at 6 s**.
- **Code era.** m3fe2 and the m3fwi rows predate `agent-stage-first`, `AGENT_CMDS_ENTRY` and `cw-link-drop`. Only m3fsf1 is on today's push path, and its front was broken. So "today's code on a kept host" has one run, and it reads 500 at 6 s, or about 496 at 5 s if the front were fixed [inference].

**What line those runs meet** [measured on their marks]. Each is the smallest bound that puts 450, 475 and 495 of 500 within it:

| Run | 450 of 500 (90%) | 475 | 495 |
|---|---|---|---|
| m3fsf1 (kept, today's code) | 5,356 ms | 5,458 | 5,778 |
| m3fhs1 (33.6) | 5,898 | 6,161 | 6,560 |
| m3fsf2 (52.5) | 6,773 | 6,875 | 7,174 |
| m3fst1 (46.4) | 7,084 | 7,121 | 7,294 |
| m3fg3 (55.8) | 17,520 | 18,263 | 18,441 |

**What each line gives up for users** [measured on the same runs; the user cost is the wait, since every push is still acknowledged or answered `full`]:
- **5 s, met at 90% on kept hosts only (as now):** in a 500-push burst on a kept host, the slowest pushes are acknowledged by 4.9 to 5.9 s. Hostguard re-places 12 of the 16 measured hosts, so most placements end without load.
- **6 s on kept hosts:** each run above passes at 495 or more. An agent waits up to about 1 s longer for its slowest burst acknowledgment. `AGENT_PUSH_BOUND` (5 s, the agent block) can stay. It sets when the edge refuses before sending, not when the slowest acknowledgment arrives, so `full` answers keep their timing.
- **7 s on hosts up to about 52.5:** m3fsf2 (492), m3fhs1 (499) and m3fpc1 (490) pass at 7 s, and m3fst1 (434) and m3fac1 (372) don't. An agent waits up to about 2 s longer. This is the only bound in the table that a re-placed host meets at 90% on today's code (m3fsf2).
- **No bound under 17.5 s meets m3fg3.** A line that holds on every host graft-m3f has drawn would let a burst push wait up to about 18.5 s for its acknowledgment.
