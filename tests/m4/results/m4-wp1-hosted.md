# M4-WP1, the hosted half, on graft-m4a

This file records M4-WP1's hosted half on `graft-m4a`, a product build with `standard-4` shards (`tests/am1/results/install-m4a.md`). The hosted half covers the `HookSequencer` Durable Object, `HookDelivery` with one automatic attempt and the `retry` opt-in, `HOOK_OWNER_CONCURRENCY`, and the leader-side `Webhooks` consumer on the M2-WP5 pipeline. The pure half is in `tests/m4/results/m4-wp1.md`.

Branch `m4-actions`. Every site day is in `install-m4a.md`'s site-day log. The raw data is in `tests/m4/results/raw/`: `bf57-runN.jsonl` holds the driver's rows, and `bf57-runN-sink.json` holds the receiver's log where it was kept. The driver is `tests/m4/wp1/bf57.mjs`.

Status: **in progress.** **BF-57 passes** (run 16, site day 18). **BF-58 passes** (run 2, site days 21 and 22). The `Audit` stream reaches a Basin Pipelines sink (site day 25). M4-WP1's hosted half is done.

## BF-57: one hook timing out at 10 s beside 49 healthy hooks, 10 events/s

The row as written:

- **Pass:** healthy hooks' first-attempt p99 at most 2 s, the slow hook gets its events in chain order, and the Webhooks cursor lag is at most 5 s.
- **Fail:** healthy p99 over 10 s, an out-of-order delivery to one hook, or cursor lag that grows with the slow hook.

### Setup

The setup has 50 hooks of owner `m3org` (account 7100001) on three repositories. `j4` has 20, `bf57b` 20 and `bf57c` 10, because a repository holds at most 20 hooks. Every hook points at the receiver Worker `graft-m4-hooksink`. j4's first hook is `?slow=1` and answers after 11 s, past the 10 s deadline.

The load is 600 commit statuses in 60 s, round robin over the three repositories. Each status's description carries a per-repository sequence number, so it is one `status` webhook to every hook of its repository.

Measures:

- **Healthy first-attempt latency.** The time from the status's 201 to the hook's first arrival at the receiver, with the receiver's clock skew taken off. The skew is the median of 9 round trips before and 9 after the load: 1.8 to 2.3 s, with the receiver ahead.
- **Order.** Per hook, chain order, by matching each arrival's `X-GitHub-Delivery` against `delivery_guid` over the chain's indexes (runs before 16 compared sequence numbers, and the GUID check for the slow hook only).
- **Cursor lag.** Each repository leader logs `webhooks cursor <repo> read <i> lag <ms>ms` on each cursor commit. The lag runs from the first entry that commit passes to the commit itself, read from Workers Logs.

The 201 is an upper bound on the entry's commit, so the latency includes the edge's reply leg.

### Runs

Each fix below was found from Workers Logs in the run before it. Runs 13 and 14's drivers lost their connection to the receiver while the slow hook drained, so their raw files hold the sent rows only; their latency and order come from the receiver's log, read afterwards (`bf57-run13-sink.json` for run 13).

| Run | Site day | Healthy p99 | Healthy delivered | Slow hook | Cursor lag | Finding and fix |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 3 | n/a | 0 | n/a | n/a | A round with nothing new sent an empty hook list, which the store read as every hook deleted, so it dropped every queued delivery. Fixed in [`83afa6d`](https://github.com/Agentifiers/graftyard/commit/83afa6d): such a round still sends the chain's hooks. |
| 2 | 3 | 526 s | 6,570 of 9,800 | 71 of 200, out of order | n/a | The site-a store kept every 11.7 KB body in its rows and in memory. It reached about 7,300 deliveries (85 MB) in 30 s, and its isolate failed inside `transactionSync` 21 times in 13 minutes ("Critical", "Reinitializing Wasm application"). `drop_chain` only marked rows done, so 699 j4 deliveries were never attempted. Fixed in [`1be2f12`](https://github.com/Agentifiers/graftyard/commit/1be2f12): bodies get their own table, only pending ones stay in memory, `drop_chain` deletes, and a report chains its queue's next attempt. |
| 3 | 5 | n/a | n/a | n/a | n/a | Restore read every old inline row at once and failed in `save`. Fixed in [`cdee31b`](https://github.com/Agentifiers/graftyard/commit/cdee31b): restore streams the rows and keeps no done delivery's body. |
| 4 | 6 | n/a | n/a | n/a | n/a | j4's consumer restarted from `replay_from` every round and moved one entry per slow-hook attempt. A stale directory also held bf57b's rounds for 26 s. Fixed in [`0196c99`](https://github.com/Agentifiers/graftyard/commit/0196c99) and [`108c1b2`](https://github.com/Agentifiers/graftyard/commit/108c1b2). |
| 5 | 7 | n/a | all | in order | bf57b 38 to 53 s | A canceled alarm invocation left `Host::alarm_at` stale, and every later `arm_in` skipped setting the alarm. Fixed in [`20b54f1`](https://github.com/Agentifiers/graftyard/commit/20b54f1): an alarm 5 s overdue is set again. |
| 6 | 8 | 6.3 s | 9,800 of 9,800 | in order | p50 about 1.7 s, spikes to 7.6 s | Stalls of 5.5 to 6.3 s came from the 5 s re-arm. Fixed in [`b10e07f`](https://github.com/Agentifiers/graftyard/commit/b10e07f): 1 s. |
| 7 | 9 | 7.1 s | 9,800 of 9,800 | in order | bf57b to 8.1 s | bf57b's leader still ran site-a version `c392af30` (site day 8) for its first 180 s. A deploy reaches a running object only when it next restarts, so this run measured the old code. |
| 8 | 9 | 2.45 s | 9,800 of 9,800 | in order (by GUID) | p50 1.6 to 2.2 s, max 7.0 s | The store's alarm ran 4 to 5 times a second, and each one ran `prune` over every delivery held (about 80,000), at about 35 ms of CPU. Fixed in [`3b7bb7e`](https://github.com/Agentifiers/graftyard/commit/3b7bb7e): prune at most once a minute. |
| 9 | 11 | 2.11 s | 9,800 of 9,800 | in order (by GUID) | p50 1.6 to 2.1 s, max 3.2 s except one 8.6 s | The output gate held each attempt's POST behind the previous reports' writes, one transaction per report. An attempt took 119 ms at p50 when it overlapped a write and 42 ms when it did not. The one 8.6 s lag passed only the cursor's own entry. Fixed in [`813b461`](https://github.com/Agentifiers/graftyard/commit/813b461) (reports written every 50 ms) and [`170e988`](https://github.com/Agentifiers/graftyard/commit/170e988) (lag measured from the first non-cursor entry). |
| 10 | 12 | 2.54 s | 9,800 of 9,800 | in order | p50 about 1.7 s; one 137 s | After the load stopped, bf57b's leader armed its alarm 2 s out, and no alarm ran after it. Five entries (bf57b 180, 20 deliveries) waited 137 s for an unrelated read to wake the member. Fixed in [`05cf164`](https://github.com/Agentifiers/graftyard/commit/05cf164): the alarm invocation sets the alarm it armed again, and awaits it, before it returns. Each 11.7 KB payload was also written 20 times per bf57b entry. Fixed in [`652ade3`](https://github.com/Agentifiers/graftyard/commit/652ade3): one body per entry and slot. |
| 11 | 13 | 2.45 s | 9,800 of 9,800 | in order | p50 about 1.7 s | Batching every 50 ms still left a write pending almost always. An attempt took 223 ms at p50 with a write pending, against 6 ms with none. Fixed in [`223d41c`](https://github.com/Agentifiers/graftyard/commit/223d41c): reports written once a second. |
| 12 | 14 | 3.18 s | 9,819 of 9,819 (601 statuses) | in order | p50 about 1.7 s | Reports written once a second made it worse, not better: 625 deliveries over 2 s against 291 in run 11. Attempt time still grows with concurrency (28 ms p50 under 10 in flight, 234 ms at 40 or more). Batching is not the lever; see below. |
| 13 | 15 | 9.4 s | 9,800 of 9,800 | in order | n/a | Attempts left the store for stateless invocations of the site script over its own `SITE` binding ([`13bbd8f0`](https://github.com/Agentifiers/graftyard/commit/13bbd8f0)). A probe found the POST takes about 205 ms at p50 from such an invocation, which runs beside the store, against 7 ms from a separate Durable Object. Replaced in [`c0f7cfd4`](https://github.com/Agentifiers/graftyard/commit/c0f7cfd4): one lane object per hook (`hooklane:<hook>`, the same class, keeping nothing) makes the POST. |
| 14 | 16 | 2.10 s | 9,800 of 9,800 | in order | n/a | All 124 deliveries over 2 s waited behind their own hook's earlier entries, one lane call per attempt. Fixed in [`0c44e818`](https://github.com/Agentifiers/graftyard/commit/0c44e818): one lane call sends its queue's run, the head and up to 8 queued deliveries, in order, stopping at the first non-2xx or after 2 s. |
| 15 | 17 | n/a | n/a | n/a | n/a | Aborted: lane objects still running day 16's code could not decode the run batch. Fixed in [`82f26e3d`](https://github.com/Agentifiers/graftyard/commit/82f26e3d): the lane's name and route carry its wire version (`LANE_WIRE`), and a lane that answers anything but 200 counts as a failed attempt. |
| 16 | 18 | **1.07 s** (max 1.78 s) | 9,800 of 9,800 | 200 of 200, in chain order | **max 4.82 s**, second half 3.29 s | **Pass.** See "Run 16" below. |

### Run 16

Site day 18 (`m4a-deploy` `8e39480b`, sites `dedb3861` / `673eba60` / `105bb420`), 2026-11-13 22:30Z, raw data `raw/bf57-run16.jsonl`.

- **Healthy first attempts.** 9,800 of 9,800, p50 326 ms, **p99 1,066 ms**, max 1,780 ms, none over 2 s. Clock skew taken off: 2,710 ms. 600 statuses committed in 70 s, with 687 sheds re-sent.
- **Chain order.** Each arrival's (index, slot) comes from its GUID: every one of the 10,000 status arrivals matches `delivery_guid` (generation 1, its repository's chain, an index up to 16,000, a slot up to 7, its hook). Per hook, first arrivals in (index, slot) order: **0 inversions on all 50 hooks**, including the slow hook's 200 of 200. No GUID arrived twice.
- **Cursor lag.** 168 cursor commits were logged. **The largest is 4,818 ms** (j4 at +17.8 s), and the largest in the second half is 3,288 ms, so it does not grow with the slow hook. Two commits at +5 s (bf57b and bf57c, lag 1,338 s and 1,334 s) passed entries committed by run 15 before this run began. They measure that earlier backlog, not this load, so the driver now leaves out a commit whose lag is longer than the time since the load began.
- **What the driver printed.** It printed "slow hook in order: false" and "hooks out of order: 50". Both are wrong for two reasons. First, it compared sequence numbers, which follow send order: concurrent writes and re-sent sheds commit in either order. Second, run 15's slow-hook queue was still draining when run 16 began on the same hooks (`KEEP_HOOKS=1`). Its 40 deliveries (indexes 4,728 to 4,754, against run 16's from 4,796) carried descriptions such as "j4 140", which read as run 16's. The receiver's full log afterwards held 240 slow-hook arrivals, 240 GUIDs. The driver now tags each run's descriptions and counts only its own arrivals, waits until the slow hook has been quiet for 30 s before it starts, and checks chain order by GUID for every hook.

### What bounds healthy latency now

The figures below are from runs 8 to 12 on site a. Healthy p99 across runs 8 to 12 was 2.45, 2.11, 2.54, 2.45 and 3.18 s, so it varies by about 1 s from run to run under the same code.

- **Where the time goes.** The time from attempt start to receiver arrival is 470 to 540 ms at p50. With no load, an attempt takes 6 ms at p50 and 20 ms at most (the 44 pings). The rest is queueing inside the one `HookSequencer` object.
- **Concurrency.** Up to 49 attempts are in flight at once. An attempt's time grows with how many others are running: in run 11, 23 ms at p50 with fewer than 10 running, and 266 ms with 40 or more.
- **Which deliveries miss 2 s.** Every healthy delivery over 2 s arrived while its queue was still busy with earlier entries, 289 of 291 in run 11. The time between two attempts of one hook is about 110 to 220 ms at p50, and the edge commits statuses in bursts behind its own sheds. So a hook that receives 5 or 6 entries within one second finishes the last of them past 2 s.
- **The lever [measured, runs 13 to 16].** Every attempt's POST ran inside the owner's one `HookSequencer` object, beside every `/record` and alarm, and batching its writes (runs 10 to 12) did not shorten attempts. Runs 13 to 16 moved the POST out: first to stateless site invocations (run 13, slower, because they run beside the store), then to per-hook lane objects (runs 14 to 16). DESIGN's Shape (`HookSequencer`) and `decisions/DECISIONS.md` ("M4-WP1's hosted half: webhook attempts run in per-hook lanes") record the result.
- **Edge sheds.** The edge sheds 40 to 55% of status writes with 503, because the members shed the pre-write read with `overloaded`. The client re-sends after `Retry-After`, so commits arrive in bursts. A status write takes 1.8 to 2.5 s at p50 to its 201.

### Order and cursor

The slow hook's deliveries arrived in chain order in every run since run 5. Matching each `X-GitHub-Delivery` against `delivery_guid` gives no inversions. In runs 8 and 9 the sequence numbers look out of order, because two statuses sent close together commit in either order. The sequence follows send order, not commit order.

Cursor lag stays around 1.6 to 2.2 s at p50 and does not grow with the slow hook. Every run's max is under 5 s, except:

- **Run 7.** Measured on the old code.
- **Run 9.** One cursor-only commit, now excluded from the measure.
- **Run 10.** The lost alarm, fixed in `05cf164`.

## BF-58: kill the site holding a hook owner's `HookSequencer` with deliveries pending

The row as written:

- **Pass:** every event in the range reaches every hook at least once, with unchanged GUIDs.
- **Fail:** any event never delivered, or a redelivery under a new GUID.

**What DESIGN promises during the outage.** A lost store is rebuilt on the owner's next site and replays from the committed `replay_from` (Shape, `HookSequencer`; `design/SHARED.md`, "Event fan-out"). The consumer already falls back to the record's next site when the home store fails and retries home every `HOME_RETRY_MS` (30 s, `workers/site/src/hookseq.rs`). So deliveries should continue from site b while site a is down, and the run must show it.

**Setup.** Owner m3org (record home site a, `enam`), so its `HookSequencer` is `hookseq:7100001` on site a. Three repositories (`bf58a`, `bf58b`, `bf58c`) with five hooks each at the receiver Worker `graft-m4-hooksink2`; `bf58b`'s first hook answers after 2 s (3 s in run 1), so its queue keeps a backlog. Commit statuses at 2/s, round robin, for 21 minutes (18 in run 1). The kill is a site day of site a alone through the production deploy path (`tests/m4/wp1/bf58-kill.sh down`: `deploy.mjs --only 0 --down 0`, `GRAFTYARD_SITE_DOWN=1`, which makes every class on site a answer 503 and run no alarm). The restore is a second site day of site a that also sets `GRAFTYARD_HOOKSEQ_RESET_BEFORE`, so every `HookSequencer` store on site a starts empty, as a store lost with its site. Driver `tests/m4/wp1/bf58.mjs`, raw data `raw/bf58-runN.jsonl`.

### Run 2 (the row): site days 21 and 22, 2026-11-13 23:52Z to 2026-11-14 00:22Z

- **Kill.** Site a's `fb5d76ea` was deployed at 23:58:11Z. Its members ran it within seconds, but the owner's store kept running day 18's version until 00:03:40Z, 5.5 minutes later, because a running object keeps its version until it restarts. `bf58-kill.sh dead` watches Workers Logs for the store's first call on the down version and marks that time. The outage was held 3 minutes from then. From 00:03:45Z site a's `HookSequencer` answered only 7 calls, all 503, and site b's answered 2,250 in the next 45 s.
- **Restore.** Site a back on `9cb4d8d7` at 00:06:51Z, with the reset.
- **Result.** **Pass.** 2,465 statuses acknowledged (55 more got 503 from the edge while site a's members went down, so no event of theirs exists). **12,325 of 12,325 (event, hook) pairs delivered, 0 missing, 0 under two GUIDs.** 1,061 pairs arrived more than once, every repeat under its first GUID.
- **During the outage.** The first delivery after the store went down arrived **0.19 s** after it, served by site b's store. 1,860 arrivals came between the store's kill and the restore, 496 of them repeats: site b's new store replayed from `replay_from`. Its first repeat arrived 201 s after the kill, once its replay reached entries site a had already delivered.
- **After the restore.** The first delivery arrived 0.95 s after site a came back, and the first redelivery at the same moment (**outage-to-first-redelivery 0.95 s** from the restore).

### Run 1 (does not count): site days 19 and 20, 23:04Z to 23:45Z

Site a's members took the down version at 23:14:11Z, but Workers Logs show its `HookSequencer` objects ran day 18's `dedb3861` through the whole 4-minute window (last call 23:19:27Z), so the store was never down. What the run did show is option A, the store reset alone: from 23:19Z the store restarted empty under `GRAFTYARD_HOOKSEQ_RESET_BEFORE` and replayed. 10,800 of 10,800 pairs were delivered, 0 under two GUIDs, 1,061 repeats after the reset, the first 0.92 s after it. This is the lead's extra check of the empty-store replay; it is not counted as the row. Run 2 adds the `dead` mark, so the outage is timed from the store's own kill.

## The `Audit` consumer and its Basin Pipelines stream

Basin Pipelines is GA in `research/cloudflare.md`'s first table and in DESIGN's Stack. Search is M4-WP2's. The `Audit` consumer (finisher) was already built in M3; this half adds its stream: each batch sends one row per event of the identity entries it scanned (`graftyard_identity::audit::stream_rows`, keyed `chain:index:slot`, variant and id fields only, never a hash, key, or secret) to an `AUDIT_STREAM` binding before it moves its cursor, so every event reaches the stream at least once (`workers/site/src/audit.rs`; kept test `audit_stream_rows_name_each_event_once_and_the_cursor_passes_them`). DESIGN folds the stream into the `event pipeline` component's Failure modes and Deployment modes rows.

**On graft-m4a.** Stream `graft_m4a_audit` (schema: `id`, `chain`, `index`, `slot`, `at`, `action`, `fields`), pipeline `graft_m4a_audit_pipe`, and an R2 JSON sink into bucket `graft-m4a-audit`, rolling every 30 s. Site days 23 to 25 turned on `GRAFTYARD_AUDIT=1` with `--audit-stream`:

- **Day 23.** Every batch failed with "audit stream send returned no promise": the binding's `send` answers an `RpcPromise`, a thenable that is not a `Promise` instance. Fixed in [`38b5bcda`](https://github.com/Agentifiers/graftyard/commit/38b5bcda) by adopting it through `Promise.resolve`.
- **Day 24.** Every batch failed with "The RPC receiver does not implement the method `call`": the binding's `send` is an RPC stub, so `Function.prototype.call` reaches the stream as a method name. Fixed in [`6abceebf`](https://github.com/Agentifiers/graftyard/commit/6abceebf) with `Reflect.apply`. A scratch probe Worker bound to the same stream showed both behaviors before each deploy.
- **Day 25.** Passes. In the first two minutes the consumers streamed the backlog from each cursor: one 5.6 MB file of 40,981 rows (40,952 `Names`, 28 `Authz`, 1 `Tokens`). 889 ids appear twice: the batches that failed on day 24 had sent some chunks before the failing one, and the cursor did not move, so they were sent again, which is the at-least-once the design states. Then `tests/m4/wp1/audit-stream.mjs` added a new SSH key for the admin at 01:07:06Z. Its `SshKey` row (`tokens:909:5:0`, `fields: {account: 7100003}`) reached the sink **144 s** after the write: `FINISH_AFTER` (60 s), the alarm, the stream, and the 30 s roll. **No row holds key material.** Raw data `raw/audit-stream-run1.jsonl`.

## Cost

`tests/fanout/m2bcost.py` for graft-m4a. It meters the `graft-m4a-*` scripts and namespaces, so it covers every thread's use of the install.

- **This work's windows, 2026-11-13 08:40Z to 2026-11-14 01:10Z: $24.12.** Containers are $13.98 of it, the `standard-4` git shards, which every thread on graft-m4a shares and none of this work started (BF-57 and BF-58 do not touch git; every shard was torn down at 20:20Z and 20:59Z). The rest is **$10.14** of Durable Objects and Workers (4.60 M DO requests, 1.60 M active seconds, 93,813 Worker requests), which also includes M4 web's, M4-WP2's, and M5-WP1's use in those hours, so this work's share is at most $10.14, under the $30 cap. The receiver Workers, the probe Workers, the stream, and the R2 sink cost cents.
- **The whole install, 2026-11-13 00:00Z to 2026-11-14 01:10Z: $25.91**, containers $14.54.
