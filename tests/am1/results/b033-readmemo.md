# b033-readmemo: B0-33 under `read-memo`, guarded, on graft-m2b

LM-1, 2026-11-23. The lead cleared one guarded B0-33 row after `read-memo` merged (main `1d1bc221`,
`decisions/DECISIONS.md`, "`read-memo`"), then two more for the median the gate is judged on (`FRAME.md`, "Operator
decision, 2026-11-22, B0-33's commit line and three-run medians"). Tags follow `design/SHARED.md`, "Read this first".

**The set, in one line.** Two runs kept (mrB, msA); the third set, mt, kept no host in three placements, which the
lead ruled is not a fail. **B0-33 fails agent throughput on both** (107.3 and 106.8/s against 126). msA's leader wasm
instance reset under load, with no panic text logged; its cause is [unverified], so the memo stays out of B0-33's
reasons ("The median set", below).

**Short answer (mrB).**
- **The memo answers 82% of the leader's fold reads hosted** [measured]: SQLite ran 10.68 reads per event applied, and
  the memo answered 49.32 of the 60.0 asked for.
- **Leader apply CPU per event fell to 0.92 ms**, against blB's 1.59 on a host the guard rates the same (setup 24.5
  against 23.7 ms a commit), pgA's 2.75 and pfA's 3.17 [measured, `heavyturns.py`]. That is 42% under blB. CPU per
  command cut fell to 3.90 ms from blB's 4.50, and leader excess p50 to 176 ms from 373.
- **On B0-33's gate, mrB passes the commit line and misses two others.** Human commit stage p99 2,603 ms is within
  3,912, and there were 0 leader changes.
  - Agent-class commands answered ok: 107.3/s against 140. That is under 90% of the line (126), so B0-33 misses on
    agent throughput even where every other line passes.
  - One merge-queue command waited 2,403 ms at the commit stage, over the fail column's 2 s.
  - 8 `suspect` reports were stepped, all from LAX and all refused (`promised`). The line reads "zero `suspect`
    reports"; A48 asks what it counts.
- **`read-memo`'s flips are not met** (below).
- **Correction to `b033-applyprice.md`:** hosted asks 60 fold reads per event, not the bed's 6.3. The bed's probe
  counted apply's reads and the renders, and never ran `decide` or the dependency reads.

## The build

| Item | Value |
|---|---|
| Site day | `-mr` from merged main `1d1bc221`, test-faults, `tests/fanout/run-siteday.sh` (siteday exit 0) |
| Sites (final deploy) | site-a `352b7093`, site-b `057aa6c3`, site-c `d7c01518` |
| Edge | graft-m2b-edge `34cfafb9`, 100% (`edgever.py`, rev `7f80acb41`) |
| Shard image | row B's `graft-m1-site-gitshard-git@sha256:db305d1e…` (no change under `containers/` since `050eefc8`) |
| Wrapper | checkout `7f80acb4` (branch `r33-memorow`): `1d1bc221` plus `a1.mjs`'s `foldReads` delta |
| Row | `guardrow.sh` TAG=mr, GUARD_WANT=fast, `--seed-prs 1000` as pgA, RUN_STOP $10, DAY_STOP $15 |

`a1.mjs` (`r33-memorow`) records the leader's `foldReads` in each row: the end `/stats` counts less those read at the
reset before load, with the events applied since that reset. A member whose handles reopened in the window is flagged
`reopened` and gives the end counts.

## Placements

- **The first reretire failed** (12:23Z, before any placement). `repo:131305#1#f1` answered a `/status` with no role or
  term in the after-check ("STILL JOINED repo:131305#1#f1 undefined tundefined"). A `/status` of all three members a
  few minutes later answered not joined, each with its `retired` row (f1's dated 11:08:53Z). The second reretire, from
  12:24:50Z, printed RERETIRE OK at 12:55:53Z [measured, `reretire-graft-m2b-mrA-first.log` and
  `reretire-graft-m2b-mrA.log`]. One member's answer in the after-check was empty, and the next read was correct
  [inference].
- **mrA, repo 131395: re-placed.** hostguard read 34.0 ms of leader CPU a setup commit (52 commits, 1,769 ms), over
  the 28.4 line, at 13:00:36Z. a1 held after its seed and exited 4 before load (`host guard: verdict "stop"`).
  - rows.sh still tore down and swept: `torn down 131395`, `{"torn_down": 6}`, "sweep: no shard container running",
    then costrow's sweep exit 0. That is the lead's check after r33-guardfix (a1 exiting 4 leaks no shards), now
    observed live.
  - Cost $0.49.
- **mrB, repo 131396: kept.** 24.5 ms a setup commit (50 commits, 1,225 ms) at 13:11:33Z, in blB's band (23.7).
  - Leader object f0 ran in **ORD**, f1 in LHR and f2 in HKG (`durableObjectsPeriodicGroups` by `coloCode`). mrA's
    re-placed leader ran in ATL. pgA's leader also ran in ORD and read 43.2 ms, so one colo gave both a fast and a
    slow host, as crA and pgA showed (A41) [measured].
  - The 1,000 PRs were seeded by 13:15:44Z. Learner gate 4 of 4, then `host guard: go`.
  - Load ran 13:15:50.5 to 13:20:50Z (a1 run `mwta9122`), done line 13:22:27Z.
  - Leader site 0 at term 2 throughout.
  - Cost $2.92. Sweep ok.

## Gate lines, mrB

[measured; `a1-b0-33-mwta9122.jsonl` and its push log, `rows-mrB.status`, `tsave-mrB`, `terms-mrB`]

| Line (B0-33, `build/BUILD-PLAN.md`) | mrB | blB (baseline) | pgA | Reading |
|---|---|---|---|---|
| Human commit stage p99, push log (n) | **2,603** (99 ok; p50 882) | 2,523 (119) | 11,018 (85) | within 3,912 |
| Merge commit stage p99 (n); merges over 2 s | 2,403 (40); **1 of 40** | 2,366 (36); 4 of 36 | 4,794 (34); 21 of 34 | over the fail column's 2 s, by 403 ms, once |
| Agent-class commands answered ok/s | **107.3** | | 55.1 | under 140, and under 90% of it (126) |
| Leader changes | **0** | 0 | 2 | pass |
| `suspect` Reported rows in the window | 8, LAX, all `promised` | 2, LAX, `promised` | 2,262, LAX | fails as written (A48) |
| Refusal shapes | api429 0 / 0; agent pushes 7,065 ok, 5,493 `full`, 2 `fetch failed` | | | `ng` and 429 shapes only |
| Flip 1 (edge-refusal) | none | | | |

- **Agent pushes refused `full`**, by stage (`pushfull.py`): edge `bound_at_send` 3,058, admission 1,140, no commit stage
  1,069, the rest expired queued. Of pushes whose first send had under 1 s of budget left, 350 were ok and 3,792 full.
- **Edge 503s: 6,077** (graphql 2,928, rest-repo-write 1,988, task 499, upload-pack 317, renew 231, lease 98, merge 9).
- **edgefull:** 84 lines from 53 isolates, 74 `refused:push` and 10 `failOpen:push`. Refused wait p50 386 ms.
- **Other:** reask-read top-ups on the leader 370; harness outages 2.0 s (0.7%, valid).
- **The one slow merge** started at 291 s and spent 2,403 of its 3,087 ms at the commit stage.

## What the memo did

[measured, leader `/stats` over the window, `a1.mjs` `leader.foldReads`]

| | mrB |
|---|---|
| Fold reads SQLite ran | 436,364 |
| Fold reads the memo answered | 2,015,292 |
| Events applied (`eventsSinceReset`) | 40,858 |
| Per event: SQLite / memo / asked | **10.68 / 49.32 / 60.0** |
| Memo share | 82.2% (handles not reopened) |
| Memo peak (memguard, every 2 s, whole row) | 81,759 bytes isolate-wide, 1,035 rows; the cap is 8 MB |
| Members' wasm peak MiB (site 0 / 1 / 2, whole row) | 82.1 / 63.8 / 74.6 (blB 92 / 37 / 46; pgA 119 / 55 / 57) |

**Leader SQL calls per event, against the matched pre-memo row.** No pre-memo hosted row counted reads, so the
pre-memo figure is this row's own asked reads (60.0 an event: the same mix and code with every read sent to SQLite)
plus pgA's 2.95 statements an event.
- Before: about 63.0 calls an event.
- After: 10.68 + 3.09 = **13.8**, a 78% drop [inference from measured].

### Leader CPU

| Run (setup ms a commit) | Apply ms an event (`heavyturns.py`) | Events, statements, KB an entry | CPU ms per command cut (`cycsplit.py`) | Leader CPU ms/s | Leader excess p50 / p90 (`second.py`) | Cycle mean (p50) | Entries/s |
|---|---|---|---|---|---|---|---|
| **mrB (24.5)** | **0.92** | 45, 139, 130 | **3.90** | 384 | **176 / 332** | 357 (329) | 2.80 |
| blB (23.7) | 1.59 | 66, 217, 95 | 4.50 | 533 | 373 / 661 | 514 (491) | 1.95 |
| pgA (43.2) | 2.75 | 56, 165, 84 | 6.58 | 362 | 536 / 1,284 | 865 (707) | 1.12 |
| pfA (46.1) | 3.17 | 49, 175, 18 | 4.96 | 275 | 643 / 1,530 | 910 (755) | 0.94 |

- **Apply share of joined CPU:** mrB 37% (blB 44%). Deferred transaction 26% at 0.21 ms a statement (blB 0.20).
- **The cycle on the near voter's clock:** wire 39%, voter 9%, leader CPU 38%, leader off CPU 14% (blB: 273 ms CPU and
  128 ms off per 514 ms cycle).
- **Throughput:** the leader cut 98.4 commands a second at 35 an entry and 2.80 entries a second, against blB's 118.3
  at 61 and 1.95. So it ran smaller entries more often, and its commit p50 was 685 ms (p99 3,790; queue p50 366, round
  p50 295) [measured, `/stats`].
- **What it means** [inference]: per event, the memo removed most of the leader's apply cost on a fast host. The agent
  line did not rise with it: a1's agent-class answers are 107.3/s against blB's 118.3 commands cut. Agent pushes still
  end `full` at the edge's send bound and in admission. So the leader's CPU is no longer what limits agent
  throughput on this host.

### `read-memo`'s flips

- **Flip 1:** "A hosted row whose leader stored reads per event fall by half or more while CPU per event falls by less
  than 15%." **Not met.** SQLite reads fell 82% (60.0 asked against 10.68 run), and apply CPU per event fell 42%
  against blB in the same setup band. This is one host pair, not a mean, and blB ran before the memo on a different
  site build.
- **Flip 2:** "A member whose memory peak rises by more than the memo's cap." **Not met.** The memo peaked at 82 KB
  isolate-wide. The leader peaked lower than blB's and pgA's. The followers peaked higher than blB's (63.8 and 74.6 MiB
  against 37 and 46), more than the cap, but not by the memo: at each follower's peak sample its memo held 0 bytes,
  its answers in flight 4.6 MB and its log 8.0 MB (memguard's `memSplit` at 13:19:06 and 13:19:13Z) [measured]. What
  else filled them is not split here.

## The median set: msA kept, mt kept no host

The lead asked for two more guarded runs, same shape as mrB, on the reused `-mr` site day (nothing on the site path
changed on main since `1d1bc221`), DAY_STOP $22 (`tests/fanout/raw/row-ms.log`, `row-mt.log`).

- **msA, repo 131397: kept on its first placement.** hostguard 22.3 ms a setup commit (49 commits, 1,093 ms) at
  14:16:21Z; leader f0 in **EWR**. Load 14:20:22.564Z to 14:25:22Z (a1 run `mwtcjqz7`), done line 14:28:07Z. Leader site
  0 at term 2 throughout the window. Cost $2.83, sweep ok.
- **mt: no kept host in three placements.** After its reretire (14:41:51 to 15:13Z, `reretire-graft-m2b-mtA.log`):
  mtA repo 131398 read 61.4 ms, mtB repo 131399 46.5 ms, mtC repo 131400 29.6 ms, all over the 28.4 line, so each a1
  ended before load (exit 4), and each rows.sh tore down and swept. The set stopped at 15:55:27Z, "third placement
  re-placed too: set stopped, swept". Per the lead's ruling this is not a fail. Cost $0.50, $0.49 and $0.51.

So the set has **two kept runs, mrB and msA**, not three. A three-run median cannot be taken; the table gives both
runs, and the set's reading below holds for either.

### Gate lines, both kept runs

[measured; `tests/fanout/raw/rows-ms{A}.status`, `tsave-msA`, `terms-msA`, `memguard-msA`, the a1 files]

| Line (B0-33) | mrB | msA | Reading |
|---|---|---|---|
| Human commit stage p99 (n) | 2,603 (99) | **2,196** (95) | within 3,912 on both |
| Merge commit stage p99 (n); over 2 s | 2,403 (40); 1 | **1,565** (49); 0 | mrB over the fail column once; msA passes |
| Agent-class commands answered ok/s | 107.3 | **106.8** | **fails on both**: under 140 and under its 90% line (126) |
| Leader changes in the window | 0 | 0 | pass |
| `suspect` Reported rows in the window | 8, LAX, `promised` | 0 | msA meets the line as written; mrB fails it as written (A48) |
| Wasm resets in the window (leader / members) | 0 / 0 | **1 / 1** (below) | new line (the lead's ruling) |
| Refusals | api429 0; push full 5,493 | api429 0; push full 4,867 | `ng` and 429 shapes only |

- **B0-33 fails the agent-throughput line on both kept runs** (107.3 and 106.8 a second against 126), whatever the
  other lines read. This is the set's verdict for agent throughput.
- msA's edge 503s: 8,333, of which 7,428 were "every member asked shed the read: overloaded" (graphql 3,652,
  rest-repo-write 2,533, task 1,012, renew 558, lease 202) and 368 git-cache `warming`.

### Leader CPU and the memo, both runs

| | mrB (24.5, ORD) | msA (22.3, EWR) |
|---|---|---|
| Apply ms an event (`heavyturns.py`) | 0.92 | 0.81 |
| CPU ms per command cut (`cycsplit.py`) | 3.90 | 3.67 |
| Leader excess p50 (`second.py`) | 176 | 159 |
| foldReads SQLite / memo an event | 10.68 / 49.32 (82.2%) | 8.68 / 77.22 (89.9%), **lower confidence** |
| Members' wasm peak MiB (0 / 1 / 2) | 82.1 / 63.8 / 74.6 | 95.8 / 68.8 / 60.4 |
| Memo peak, isolate-wide | 81,759 B, 1,035 rows | 105,706 B, 1,178 rows |

msA's foldReads are lower confidence: the leader's wasm instance reset at 14:23:22Z, mid-window, and a1's end read
does not flag a reopened handle for a re-instantiated module, so its counts cover an unknown share of the window. The
memo share the set relies on is **mrB's 82.2%**; mt gave no run to pair with it.

### msA's resets, traced at $0

[measured from Workers Logs, `wl-ms/msA-f0-msgs.txt.xz` and `msA-f1-msgs.txt.xz` from `tests/fanout/wlmsg.py`, and
memguard's 2 s samples via `tests/fanout/resets.py`]

- **In the window:** the leader f0 at 14:23:22Z (last sample 64.6 MiB, next 15.9) and follower f1 at 14:24:51Z (33.6 →
  10.4). **After it:** f0 again at 14:25:48Z (95.8 → 9.8), then a loop: from 14:25:48 to the end of the fetch at
  14:28:05 the leader's alarm ended `exception` 60 times, about every 4 s, 41 of them after 300 to 580 ms of CPU, while
  site 0 flapped between leader and follower at term 2 (`terms-msA`). mrB and every earlier B0-33 run on graft-m2b (blA,
  blB, blC2, qlA to qlC, pfA, pgA) show no reset.
- **The first throw.** Leader alarm invocation `3a7b00f5` ran 293 ms of CPU in 656 ms of wall time (alarms p50 0 ms,
  p99 24 ms in the window). At 14:23:22.514 it and the 21 `/commit` fetches held on the member logged "Promise will
  never complete.", from 14:23:22.573 "Cannot invoke closure from previous WASM instance" (`index.js:1900`,
  wasm-bindgen's closure guard), then the alarm and the 21 commits ended `exception`. The JS isolate stayed; the wasm
  instance under it was re-created.
- **No panic text.** No "Critical", "Rust panic", "RuntimeError", "unreachable" or "Reinitializing Wasm application"
  line exists in f0's whole window (14:20:20 to 14:28:05, 1,490 console or non-ok events), nor around f1's reset.
- **Why none was logged** [verified in the build, `workers/site/build/index.js`, `worker-0.8.7/src/init.rs`,
  `worker-build-0.8.7/src/js/shim.js`]: the panic hook logs `Critical` and "Rust panic: …" for a Rust panic, and the
  shim logs `Critical` for a `WebAssembly.RuntimeError` that reaches its entry proxy. But the glue's catch around every
  call into wasm (`o()`) marks the instance terminated and schedules a re-init for any other throw, with no console
  line, and the next call re-instantiates (`$()`). A trap or abort inside a future's poll, which runs from a promise
  callback and not through the proxy, resets the instance silently. Under `panic = "abort"` an allocation failure
  aborts without the panic hook. So the resets fit a wasm trap or abort, such as a failed allocation, and not a Rust
  panic [inference]. Workers Logs keeps console lines, so a panic would have shown; nothing in the build logs an abort.
- **Is the memo on the path?** Its memory is not: the memo held 0 bytes and 0 rows on the leader in every sample from
  14:23:14 to 14:23:21, peaked at 105,706 bytes isolate-wide over the row, and is capped at 8 MB [measured]. Whether
  its code was on the trap's stack cannot be read without the trap's text [unverified]. The alarm that trapped runs no
  memo-specific code beyond tier reads, which every fold read takes.
- **A candidate, not settled** [hypothesis]: the alarm that trapped, and each one in the loop, burned 250 to 580 ms of
  CPU, and after an instance reset the leader's in-memory consumers (`prsync::DISPATCHERS`, `merge_queue::LEADS`)
  start again at their committed cursors and re-scan stored rows (up to `SCAN_BYTES`, 16 MB, decoding inline packs). In
  msA every `PrSync` batch after 14:21:31 was shed (`"shed": 54` of 54 PRs), so the cursor trailed. The evidence missing
  is the alarm's step split for the trapping alarms: `alarm_steps` logs only after the alarm returns, and these did not.
- **Follower f1's reset** came while it wrote a snapshot: its alarm logged "snap 1292 to 1791 ms" every 2 to 6 s from
  14:24:31, and the leader received 52 `/snapshot_commit` in msA against 0 in mrB.

**What this means for B0-33's reasons.** mt did not load, so it neither repeated nor cleared msA's resets. Under the
lead's ruling the resets stay [unverified] as to cause. One of two kept runs reset its leader under load with the memo
on and mrB did not; that does not show the memo caused it, and the trace above does not clear its code either.

## Correction to `b033-applyprice.md`: hosted asks 9.5 times the bed's reads

`b033-applyprice.md` priced hosted apply per SQLite call from the bed's 6.3 fold reads per event (471 an entry at N =
60, `readprobe.patch`). mrB's leader asked **60.0 reads per event**: 2,965 an entry, 74.2 a command.
- **What the probe counted.** `readprobe.patch` counted reads in the bed's stages: apply, the done-reply render, the
  consumers' scans and the feed ring.
- **What the hosted counter counts.** `foldReads` counts every read of the fold's two tier handles in the member's
  isolate, whatever asks.
- **What the bed never runs** [verified in code, `tests/leaderprof/bed/src/lib.rs`]:
  - `decide` for any command. It builds each entry's events directly (`entry()`). Every hosted push's `decide` runs
    `may_write`, the `LeaseView` reads, `pulls_from` per pushed ref, and `PathSet`'s planned leases through
    `RepoScratch`, whose `Overlay` falls through to the tier handle on every miss (`crates/fold/src/store.rs`,
    `Overlay::get` and `scan`).
  - Every REST and GraphQL write's `decide` (labels, comments, task edits, leases, renews, draft PRs).
  - The leader's tier reads for `/read`, `/outcome` and `/outcome_by_request`, which mrB's leader served 1,270, 7,888 and
    934 times.
- **So the bed counted** apply's and render's reads only, about a ninth of what the leader asks [inference]. The per-call
  price stays a division: `b033-applyprice.md`'s 0.13 to 0.58 ms "a read" is CPU divided by 6.3 reads an event. At
  60 an event the same CPU is 0.014 to 0.06 ms a read, the bed's order (≤0.064).
- **What changes.** `b033-applyprice.md` said hosted calls cost 2 to 9 times the bed's. With the read count corrected,
  hosted apply is about the bed's cost per call times about ten times the calls. The memo's result fits the corrected
  count, not the old one: it removed 82% of 60 reads an event, and CPU per event fell 42% [inference].
- **The B0-33 mix's expected effect** in DECISIONS (`read-memo`, about −40% SQL calls) undercounted for the same reason.
  Hosted, the cut was −78%.

## Raws and tools

- `tests/fanout/raw/`:
  - `costrow-mr{A,B}.status`, `rows-mr{A,B}.status`, `terms-mr{A,B}.jsonl`, `memguard-mr{A,B}.{err,jsonl.xz}`;
  - `tsave-mr{A,B}.jsonl.xz`, `arrivals-mrB.jsonl`, `shardgate-mr{A,B}-b0-33-{0,1,2}.jsonl`, `edgefull-mrB.jsonl`;
  - `reretire-graft-m2b-mrA-first.log` (the failed pass) and `reretire-graft-m2b-mrA.log` (mrB follows a re-place, so it reretires nothing);
  - `siteday-mr.status` (account redacted), `row-mr.log` (the set's wrapper log, both service starts);
  - `wl-mr/mrB-f{0,1,2}.json.xz`: the three voters' Workers Logs, 13:14:50 to 13:21:30Z, from `wlobj.py fetch`.
- The median set, in `tests/fanout/raw/`: msA's `costrow`, `rows`, `terms`, `memguard` (.xz), `tsave` (.xz),
  `arrivals`, `shardgate` and `edgefull` files; mt{A,B,C}'s `costrow`, `rows`, `terms`, `memguard`, `tsave` and
  `shardgate` files; `reretire-graft-m2b-mtA.log`; `row-ms.log` and `row-mt.log`; `wl-ms/msA-f{0,1,2}.json.xz` (the
  three voters' invocations, 14:19:20 to 14:26:00Z) and `wl-ms/{msA-f0,msA-f1,mrB-f0}-msgs.txt.xz` (console lines and
  non-ok invocations with their messages).
- `tests/fanout/resets.py TAG [FROM TO]` lists memguard's wasm resets (a drop of 20 MiB or more between samples);
  `tests/fanout/wlmsg.py SERVICE NAME FROM TO` prints one object's console lines and non-ok invocations.
- **The a1 raws stay local** (`tests/am1/raw/` is git-ignored): `a1-b0-33-mwta9122.jsonl`, its push log, and
  `runs/a1-b0-33-fmr{A,B}-b033.log`.
- `tests/fanout/b033probes/host/common.py` and `tests/fanout/heavyturns.py` gain mrB.
- **The row's last two steps ran by hand.** `edgefull` and the top-up count ran at 13:35Z, after the orb service was
  stopped before `b033.sh` reached them. `rows-mrB.status` says so where they start.
