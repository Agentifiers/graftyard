# M4-WP1, the hosted half, on graft-m4a

This file records M4-WP1's hosted half on `graft-m4a`, a product build with `standard-4` shards (`tests/am1/results/install-m4a.md`). The hosted half covers the `HookSequencer` Durable Object, `HookDelivery` with one automatic attempt and the `retry` opt-in, `HOOK_OWNER_CONCURRENCY`, and the leader-side `Webhooks` consumer on the M2-WP5 pipeline. The pure half is in `tests/m4/results/m4-wp1.md`.

Branch `m4-actions`. Every site day is in `install-m4a.md`'s site-day log. The raw data is in `tests/m4/results/raw/`: `bf57-runN.jsonl` holds the driver's rows, and `bf57-runN-sink.json` holds the receiver's log where it was kept. The driver is `tests/m4/wp1/bf57.mjs`.

Status: **in progress.** BF-57 does not pass yet: chain order and cursor lag pass, but healthy first-attempt p99 is 2.1 to 3.2 s against 2 s. BF-58, `Audit`, and the DECISIONS and BUILD-PLAN entries are not written yet.

## BF-57: one hook timing out at 10 s beside 49 healthy hooks, 10 events/s

The row as written:

- **Pass:** healthy hooks' first-attempt p99 at most 2 s, the slow hook gets its events in chain order, and the Webhooks cursor lag is at most 5 s.
- **Fail:** healthy p99 over 10 s, an out-of-order delivery to one hook, or cursor lag that grows with the slow hook.

### Setup

The setup has 50 hooks of owner `m3org` (account 7100001) on three repositories. `j4` has 20, `bf57b` 20 and `bf57c` 10, because a repository holds at most 20 hooks. Every hook points at the receiver Worker `graft-m4-hooksink`. j4's first hook is `?slow=1` and answers after 11 s, past the 10 s deadline.

The load is 600 commit statuses in 60 s, round robin over the three repositories. Each status's description carries a per-repository sequence number, so it is one `status` webhook to every hook of its repository.

Measures:

- **Healthy first-attempt latency.** The time from the status's 201 to the hook's first arrival at the receiver, with the receiver's clock skew taken off. The skew is the median of 9 round trips before and 9 after the load: 1.8 to 2.3 s, with the receiver ahead.
- **Order.** Per hook, from the sequence numbers. For the slow hook, chain order is checked by matching each arrival's `X-GitHub-Delivery` against `delivery_guid` over the chain's indexes.
- **Cursor lag.** Each repository leader logs `webhooks cursor <repo> read <i> lag <ms>ms` on each cursor commit. The lag runs from the first entry that commit passes to the commit itself, read from Workers Logs.

The 201 is an upper bound on the entry's commit, so the latency includes the edge's reply leg.

### Runs

Each fix below was found from Workers Logs in the run before it.

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

### What bounds healthy latency now

The figures below are from runs 8 to 12 on site a. Healthy p99 across runs 8 to 12 was 2.45, 2.11, 2.54, 2.45 and 3.18 s, so it varies by about 1 s from run to run under the same code.

- **Where the time goes.** The time from attempt start to receiver arrival is 470 to 540 ms at p50. With no load, an attempt takes 6 ms at p50 and 20 ms at most (the 44 pings). The rest is queueing inside the one `HookSequencer` object.
- **Concurrency.** Up to 49 attempts are in flight at once. An attempt's time grows with how many others are running: in run 11, 23 ms at p50 with fewer than 10 running, and 266 ms with 40 or more.
- **Which deliveries miss 2 s.** Every healthy delivery over 2 s arrived while its queue was still busy with earlier entries, 289 of 291 in run 11. The time between two attempts of one hook is about 110 to 220 ms at p50, and the edge commits statuses in bursts behind its own sheds. So a hook that receives 5 or 6 entries within one second finishes the last of them past 2 s.
- **Next lever [inference].** Every attempt's POST runs inside the owner's one `HookSequencer` object, which serves up to 49 of them at once beside every `/record` and alarm. Batching its writes (runs 10 to 12) did not shorten attempts. DESIGN's Shape (`HookSequencer`) has stateless delivery Workers run the attempts. The next change is to send each attempt's request to a stateless invocation of the site script, as `prsync::in_band` does, and keep only the bookkeeping in the object.
- **Edge sheds.** The edge sheds 40 to 55% of status writes with 503, because the members shed the pre-write read with `overloaded`. The client re-sends after `Retry-After`, so commits arrive in bursts. A status write takes 1.8 to 2.5 s at p50 to its 201.

### Order and cursor

The slow hook's deliveries arrived in chain order in every run since run 5. Matching each `X-GitHub-Delivery` against `delivery_guid` gives no inversions. In runs 8 and 9 the sequence numbers look out of order, because two statuses sent close together commit in either order. The sequence follows send order, not commit order.

Cursor lag stays around 1.6 to 2.2 s at p50 and does not grow with the slow hook. Every run's max is under 5 s, except:

- **Run 7.** Measured on the old code.
- **Run 9.** One cursor-only commit, now excluded from the measure.
- **Run 10.** The lost alarm, fixed in `05cf164`.

## Cost

`tests/fanout/m2bcost.py` for graft-m4a, 2026-11-13 00:00Z to 20:00Z: **$22.13** for the whole install. Containers are $13.19 of it. They are the `standard-4` git shards, which every thread on graft-m4a uses; BF-57 does not touch git. Durable Objects are 5.08 M requests and 1.67 M active seconds. The figure covers M4 web's, M4-WP2's and M5-WP1's use of the install too.
