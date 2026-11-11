# Idle git cache shards on graft-m2b: what wakes them, and the fixes (2026-11-11)

Branch `idle-shards`, from `origin/lm1-memory` at `80d8837` (main plus the member read cap), so its site day rolls
nothing back. Only `graft-m2b` was used. `graft-m1-*` and `graft-m3c-*` were not touched. Nothing is merged.

## Summary

- **Before, measured:** with every network shard torn down at 03:40Z and no load, 23 shard containers were running
  again by 04:45Z: 20 of the networks torn down at 03:40Z, and 3 new ones (section 1). Container cost in the window
  $1.11 of the install's $1.40 (`m2bcost.py`, 03:41 to 04:45Z).
- **Cause 1, measured: a rehydrate that can never finish.** A shard with no repository rehydrates. It loaded every
  marked stored pack before the log's inline packs. On these networks the first stored pack is thin against an
  earlier inline pack, so `index-pack` failed with "did not receive expected object" every time. Every caller of that
  shard failed the same way and came back.
- **Cause 2, measured: `PrSync` re-sends without a bound.** 2,977 `shard/merge` calls in 64 idle minutes, every one
  `exception` or `canceled`. The leader re-sends unmoved work every `REDELIVER_AFTER_MS` for as long as it leads, and
  the queue consumer retried each failed message 10 more times.
- **Cause 3, measured: a new leadership's `warm`.** Groups with no client re-elect when their members idle out.
  Every new leadership's `warm` posts `shard/catchup`, which started a stopped container, and counted as a request,
  so the shard then stayed warm `WARM_FOR` (24 h).
- **The fold audit, not a cause by itself [measured].** Its `/fold` call wakes one old leader per run per site. The
  waking leader's first alarm then runs `warm` (cause 3), which is how 327680 to 327682 started at 04:33Z. With cause
  3 fixed, the audit wakes members but no shard.
- **Fixes:** (1) `rehydrate` leaves a marked pack that a chain's tail names to the tail, in index order; (2) the
  dispatcher parks a chain after `PARK_AFTER` (10) batches from one cursor, the member stores the park, and the
  consumer acks a failed re-send; (3) `warm` starts no stopped shard and does not count as its last request.
- **After, measured** (section 5, site day of `d8d6a35`): over 60 idle minutes, 07:38 to 08:38Z, 0 shards started (30
  samples), 0 `shard/merge` calls, and the fold audit's 14 `warm` calls started nothing. Container cost $0.06 against
  $1.05 in the same length of window before, and all of it from one instance the Containers API did not list
  (131210 on site c, warm since November 10), torn down at 08:43Z. A fresh repository's push and pull request still got
  its shard and its test merge within 5 s.
- **Upgrade leftovers (section 5.1):** the old release's `PrSync` queue backlog (≈9,000 messages, some 3.5 h old) ran
  test merges for about 15 minutes after the site day, until the queues were purged and the consumer invocations that
  held messages had ended.

## 1. Before: one idle hour on `d9ee594` (03:40 to 04:45Z)

At 03:40Z `teardown.py --all-nets --pools` tore down every running shard: 31 containers, 24 network shards, 7 pool
shards, all answering 200. Nothing pushed to `graft-m2b` until 04:45Z (T-01a1c694's rows were held for this window).
`tests/fanout/idlewatch.sh before` read the Containers API every 2 minutes (`raw/idlewatch-before.log`).

| UTC | Running | Networks |
| --- | --- | --- |
| 03:41:35 | 4 | 131211, 131212, 131216 |
| 03:47:45 | 9 | + 131217, 131221, 131222, 131226 |
| 03:53:55 | 13 | + 131214, 131215, 131224, 131225 |
| 04:06:17 | 19 | + 131218, 131219, 131220 |
| 04:18:38 | 20 | + 131223 |
| 04:37:09 | 23 | + 327680, 327681, 327682 |
| 04:43:19 | 23 | the same |

Shard calls in the window (Workers Logs, `count.py`-style `calculations`, URLs folded):

| Site | `shard/merge` | of which `exception` / `canceled` | `shard/catchup` |
| --- | --- | --- | --- |
| a | 1,907 | 940 / 27, the rest still running at the window's end | 5 |
| b | 734 | 350 / 34 | 3 |
| c | 336 | 162 / 12 | 2 |

The merges, by network: site a 131211 784, 131216 627, 131212 266, 131222 162; site b 131212 586, 131225 102; site c
131210 300. Each `PrSync` error in the consumer's log is the same line, for example:

```
prsync 131216 0..198: Error: ingest 3abb04ee…: 422 git index-pack: exit status 128: fatal: did not receive expected object da028eab…
```

Cost, `m2bcost.py 2026-11-11T03:41Z 04:45Z`: containers $1.11 (1,225 CPU-s, 412,403 GiB-s, 824,806 GB-s), install
total $1.40.

## 2. Cause 1: the rehydrate's order

Evidence, for 131216 (131211 and 131212 have the same shape):

- The pack named in the error, SHA-256 `3abb04ee…`, is the stored pack in slot 3 of log entry 125, the network's first
  stored pack (`/_bed/log`, 206 entries). Its `Committed` marker in `graft-m2b-site-a-packs` names index 125.
- The missing object `da028eab…` is `refs/heads/main`'s commit, which entry 3's inline pack brought. 131211's
  first stored pack (entry 123, `4ac3a588…`) misses `7d332859…`, entry 3's tip, and 131212's (entry 122, `a601a528…`)
  misses `b4a29814…`, also entry 3's.
- Offline, all 423 packs of 131216, fetched from the bucket and the log: ingested in log order into an empty bare
  repository with `index-pack --strict --fix-thin`, all 423 succeed. Entry 125's pack first fails with exactly
  `fatal: did not receive expected object da028eab985c992aace373f63e8e86d539448002`.
- `ShardHost::rehydrate` (`crates/git/src/host.rs`) ingests every marked pack, sorted by index, before it reads the
  tail. None of these networks ever promoted, so the tail is the whole log, and the marked phase loaded entry 125
  before entry 3.

`ShardHost::ingest` retries only a 409 ("unresolved delta" or "missing"). This failure is a 422, so the rehydrate
aborts, and the next call starts it again. A test merge needs the shard caught up, so every `PrSync` re-send of these
networks started a stopped container to fail the same way.

**Fix** (`crates/git/src/ingest.rs`, `rehydrate_plan`; `host.rs`): the marked phase takes each chain's last
`Promoted`, and leaves a marked pack at or after that chain's tail start to the tail, which loads every pack in index
order. The last promoted pack stays in the marked phase, because the tail's first packs may build on it. The design's
statement of `rehydrate` (DESIGN, Shape, "Packs") does not change; the order is now stated there.

Test `a_rehydrate_leaves_the_tails_stored_packs_to_the_tail_in_index_order` drives the same two phases `host::rehydrate`
runs, for 131216's shape (inline at 3, stored at 125) and for a promoted range. Planted bugs 5 to 7 (section 4).

## 3. Cause 2: `PrSync` re-sends without a bound

- `Dispatcher::batch` (`crates/git/src/prsync.rs`) re-sends work the cursor has not passed every `REDELIVER_AFTER_MS`
  (30 s), and a failed first attempt at once, for as long as the member leads. Nothing counted attempts.
- The consumer (`workers/site/src/prsync.rs`, `consume`) called `m.retry()` on a failed batch, and the queue retries
  10 times, 2 s apart (`workers/site/deploy.mjs`). One re-send became up to 11 merges.
- `design/SHARED.md`, "Event fan-out", Isolation already says what should happen: "A handler that fails 10 times on one
  entry parks that chain for that consumer, and an operator command replays it. Oldest pending work and parked chains
  per consumer are SLO signals." The code did not do it.

**Fix:**
- `PARK_AFTER = 10`. The dispatcher counts batches, first attempts and re-sends, handed out from one committed cursor.
  At 10 it hands out none, and `waiting` stops re-arming the alarm. A moved cursor resets the count.
- The member stores the park (`prsync_park`), so the next leadership on that member restores it with
  `Dispatcher::park` instead of spending 10 more attempts. Members re-elect often on an idle group, so without this a
  stuck chain would still cost 10 attempts per leadership.
- The park is visible: an error in the member's alarms and logs ("prsync N parked at C after 10 batches") and
  `prsyncParked` in `/stats`.
- The operator's replay: member route `/prsync {"replay": true}` clears the stored park and the dispatcher, so the next
  alarm starts again from the cursor. A cursor moved by any commit also unparks.
- The consumer acks a failed `PrSync` or `MergeQueue` message instead of retrying it: the dispatcher's re-send is the
  retry, and it is the one that counts toward the park.

Tests in `crates/git/tests/prsync.rs`: `a_batch_that_fails_every_time_is_parked_after_park_after_attempts` (exactly 10
batches over 30 re-send periods, parked and not waiting, more pushes send nothing, a restored park sends nothing, a
replay moves the cursor and the pipeline syncs again) and `a_healthy_pipeline_never_parks_however_many_batches_it_sends`
(30 pushes, all synced). Planted bugs 0 to 4.

## 4. Cause 3: a new leadership's `warm`

- Workers Logs, 03:41 to 04:45Z: 14 `shard warm` lines on site a, 2 on b, 1 on c, all from member alarms or reads on
  groups no client touched, most of them failing on cause 1.
- 327680 to 327682 (older installs' repositories): the `*/15` cron at 04:32:5xZ called `/fold` on their members
  (the fold audit's leader and voter reads), the woken group elected, and its leader's first alarm ran `warm`, which
  started each shard: `shard warm 327680 1285ms` at 04:32:55.010, container running at 04:33:01.
- Each group's log shows the elections: 131211 committed term 9's `Noop` pairs at 22:49, 00:08, 00:14, 01:42 and
  02:35Z with no other entry.
- `warm` (`tests/m2/results/m2-wp6.md`, cut 2) exists for BM-2: a freshly elected leader's first merge otherwise caught
  up first, 270 to 540 ms. The design states merges run on the leader site's shard and that `PrSync` and `MergeQueue`
  keep it caught up; it does not ask a new leadership to start a stopped shard.

**Fix** (`graftyard_git::serve`, `workers/site/src/shard.rs`): `warm` calls `/catchup?warm=1`. A warm call on a stopped
container answers `{"skipped": "stopped"}` and starts nothing. On a running one it catches up as before, but it does
not `touch`, so it never extends the shard's `WARM_FOR`. Every other call is a client's or caused by one, and starts
and touches as before. BM-2's case, a leader elected while its shard serves, is unchanged.

Test `only_a_clients_call_starts_a_shard_or_keeps_it_warm` (`crates/git/tests/shard.rs`). Planted bugs 8 and 9.

The fold audit itself needs no change: with `warm` fixed, the members it wakes start no shard. The archive alarm's
promotion still starts its shard, as the design requires ("Before the leader archives a range, the home shard packs
the objects ..."), but only for a log over `LOG_MAX_BYTES`, which no idle test repository has. Its catch-up runs with
cause 1 fixed.

## 5. After

Site day of `d8d6a35` (this branch on `origin/lm1-memory` at `80d8837`), 06:39 to 06:50Z, `NOROLL=1`, test-faults, the
same container image: sites a `ac6277e8`, b `70886c03`, c `e0bb45c0`; edge `187c9ad8`
(`tests/am1/raw/runs/fanout-idle-{siteday,edge}.log`, `raw/siteday-idle.status`: exit 0).

### 5.1 The queues' backlog, before the idle hour

The first attempts at an idle window, at 06:54Z and 07:02Z, did not stay idle (`raw/idlewatch-after-backlog.log`,
`raw/idlewatch-after-drain.log`): 6 to 17 shards started within minutes. All of them came from messages the old
release had put on the `PrSync` queues, not from the new release's leaders:

- The three `PrSync` queues held a backlog that had grown since about 18:00Z on November 10: a ≈5,000 messages, c
  ≈3,000, b ≈1,500 (GraphQL `queueBacklogAdaptiveGroups`). Some messages had been queued for 3.5 h. The old consumer
  retried each failed message 10 times, and the old leaders kept adding re-sends every 30 s, so the queue grew faster
  than its consumer, which ran one test merge at a time, could drain it.
- The new consumer read these old messages. Most answered `{"skipped": C}` at once, because the cursor had passed
  them, but each batch's first message for a chain whose cursor had not moved ran its merges: for example
  `prsync 131221 156..372 ... handled 844952ms {"events":0, "prs":92}`, about 850 `shard/merge?repo=131221` calls per
  minute on site a, all `ok`, from consumer invocations that ran 760 to 900 s (the queue consumer's wall limit).
- A purge at 07:01Z did not remove the messages those invocations held. When the invocations ended or hit
  `exceededWallTime` (07:08 to 07:16Z), their messages went back on the queue (a ≈4,900, c ≈2,900 at 07:10Z), and the
  next invocation ran them again. The last such merge, `shard/merge?repo=131228` at 07:19:06Z on site c, belonged to a
  trace whose invocation hit the wall limit at 07:16:19Z.
- Purged again at 07:12Z and 07:21Z (`POST /queues/<id>/purge`, `delete_messages_permanently`). From 07:21Z the
  queues read 0 messages and no write.
- One more leftover: a teardown destroys a container, but not the calls already waiting in its shard object. Trace
  `b4306e13`, a site c consumer invocation from before the purge (`exceededWallTime` at 07:16:13Z), had left a
  `shard/merge?repo=131228` (canceled at 07:19:06Z) whose catch-up rehydrated 131228 from the log
  (`member/entries?from=2...`). The container was torn down at 07:16Z and at 07:23:17Z, and the waiting catch-up
  started it again each time: `started_at` 07:23:34.6Z, with no shard request logged after 07:19:06Z. Its last log
  read was at 07:31:47Z. After one more teardown at 07:38Z nothing was left.
- Meanwhile the new release parked every chain it could not finish, each after exactly 10 batches, logged once:
  `prsync 131228 parked at 120 after 10 batches` (06:58:42Z, site c), and 131210, 131223, 131227, 131230, 131232,
  131233 likewise. No leader re-sent a parked chain afterwards: queue writes fell from 10 to 26 a minute per site at
  06:51 to 06:57Z to 1 or 2 by 07:03Z, and to 0 after 07:06Z.

This backlog is the old release's leftover, and an install upgrading from it carries the same queue. A purge after
the site day is part of the upgrade on `graft-m2b`. Without a purge the backlog still ends, because the new consumer
acks every message it reads and the new leaders add nothing for a parked chain, but each stale message whose cursor has
not moved runs its merges once more first: on `graft-m2b` that was about 15 minutes of shard work per site.

### 5.2 The idle hour

At 07:38:17Z `teardown.py --all-nets --pools` left 0 shards running (the last one, 131228, from 5.1), and the queues
were empty. Nothing was pushed or loaded until 08:38:17Z; T-01a1c694 held its rows and reapers.
`tests/fanout/idlewatch.sh after` read the Containers API every 2 minutes (`raw/idlewatch-after.log`).

| | Before (`d9ee594`, 03:41:31 to 04:41:31Z) | After (`d8d6a35`, 07:38:17 to 08:38:17Z) |
| --- | --- | --- |
| Shards running at the end, of 0 at the start | 23 | 0 (30 samples, every one 0) |
| `shard/merge` calls | 2,977 (64 min), every one `exception` or `canceled` | 0 |
| `shard/catchup` calls | 10 | 14, all `warm=1`, all `ok` in 10 to 19 ms: `skipped`, no start |
| Container GiB-s (`containers.py`) | 388,225 | 22,528, all from one unlisted instance (below) |
| Container cost | $1.05 | $0.06, the same instance |
| Install cost (`m2bcost.py`) | $1.34 | $0.08 |

- The 14 warm calls came from the fold audit's `*/15` cron waking old groups (131137 to 131147, 131233): exactly the
  path that started 327680 to 327682 before. Now each leader's `warm` found the shard stopped and left it so.
- Queue writes: 0 on all three `PrSync` queues in the hour. Parked chains stayed parked.

**An instance the Containers API does not list.** `containersUsageAdaptiveGroups` by `instanceId` billed one instance
on `graft-m2b-site-c-gitshard`, `b4380068…`, through both windows (22,078 GiB-s before, 22,528 after: one 6 GiB
container), while `GET /containers/applications/<id>/instances` listed 46 instances, all stopped, and not this one.
The instance id is the shard object's id; Workers Logs name it as the shard of 131210 (`git#2#net:131210`): 1,969
failing `shard/merge?repo=131210` calls since 14:22Z on November 10 (cause 1), 169 teardowns until 23:47Z, then none,
because `teardown.py` only sees listed instances. It had billed every hour since 13:00Z November 10, about $0.06 an
hour. `/_bed/shard_status?repo=131210&shard=2` answered `{"running": true}`. `/_bed/kill?repo=131210&shard=2` at
08:43:47Z stopped it: its billing ends in minute 08:43, and the list now shows it as `stopped`. The code needs no
change for this: its last request was a backlog test merge at 06:53Z, a client-caused call that keeps a shard warm
`WARM_FOR` by design. A teardown that trusts the instance list can miss an instance, so `tests/fanout/ghosts.py
FROM TO` names every instance billed in a window, listed or not.

**Active control** (`tests/idleshards/active.mjs`, `raw/idle-active-ctl3.log`), 08:44Z after the idle hour: a fresh
repository (131236), a push to `topic`, a pull request (the leader's test-faults `openPr`, because the edge serves no
`POST /pulls`), and a head push. `refs/pull/1/head` was the new head and `refs/pull/1/merge` existed 5 s after the head
push, so the network's shard (`git#0#net:131236`) started and computed the test merge. It was the only running shard.
An earlier control just after the site day (131235) gave the same result.

## Plants

`python3 tests/idleshards/plant.py`: 10 of 10 caught.

| # | Planted bug | Caught by |
| --- | --- | --- |
| 0 | the dispatcher never parks | `a_batch_that_fails_every_time_is_parked_after_park_after_attempts` |
| 1 | the count never resets when the cursor moves | both park tests |
| 2 | `PARK_AFTER + 1` batches go out | the park test |
| 3 | a parked chain keeps re-arming the alarm | the park test |
| 4 | a restored park is ignored | the park test |
| 5 | the marked phase loads every marked pack first, as before | `a_rehydrate_leaves_the_tails_stored_packs_to_the_tail_in_index_order` |
| 6 | the promoted pack is left to the tail too | the same |
| 7 | the tail starts at the promoted range's first index | the same |
| 8 | a warm call starts a stopped shard | `only_a_clients_call_starts_a_shard_or_keeps_it_warm` |
| 9 | a warm call keeps the shard warm | the same |

## Teardown

`tests/fanout/teardown.py` takes several `FIRST LAST` ranges, or `--all-nets` for every network shard, so older
installs' networks (65536 to 65540, 131072 to 131073, 327680 to 327682) are covered. It sees only instances the
Containers API lists; `tests/fanout/ghosts.py FROM TO` finds billed instances the list misses (section 5.2).

At the end (08:48Z) every graft-m2b shard was torn down, including the control's 131236.

## Cost

`m2bcost.py`, graft-m2b only, overage prices:

| Window | Containers | Install |
| --- | --- | --- |
| Before: 03:39:14 to 04:45Z (baseline hour) | $1.19 | $1.49 |
| Site day, backlog drain, idle hour, control: 06:39:38 to 08:47:31Z | $1.07 | $1.36 |
| Total | $2.26 | $2.85 |

Of the second window, the idle hour itself was $0.08. The rest is the site day and the backlog's merges before the
purges (section 5.1). Builds ran on the orb. Within the $8 cap.
