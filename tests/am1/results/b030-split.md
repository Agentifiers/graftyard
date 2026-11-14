# b030-split: the metadata child group, and B0-30 rerun with pushes on

Branch `b030-split`, on `graft-m1`. Gate (`build/BUILD-PLAN.md`, B0-30): member SQLite under `CHILD_GROUP_AT`
(8,589,934,592 B) with zero `SQLITE_FULL`, and archived entries readable for `RestoredTo`, over 30 simulated days of
journey 9 with pushes on, on a group seeded with the 4 GB metadata tier.

**Status: built and tested in memory. Smoke 1 split a real group. The 30-day rerun has not run yet.**

## What is built

- **The split** (`5ae67e0`, `81d5c2f`). At `SPLIT_AT` (7 GiB of `databaseSize`) the parent leader records
  `MetaSplit { child: RepoMeta(repo) }`. Then it records `MetaSeed { at: p, key }` at its newest committed metadata
  snapshot, creates `repo-meta:<id>`, and seeds it. Each child member restores the snapshot. The parent then records
  `MetaDropped` and `MetaSlim` until one pass is done. Reads fill slimmed text from the child. Every table `decide`
  reads stays on the parent, so commands, `RestoredTo`, consumers, and the log stay there as well (`decisions/DECISIONS.md`,
  "A split moves only the text `decide` never reads"). There is no `crates/raft` change and no new `IdentityDep`.
  Wire enums are appended only (`crates/git/tests/wire_tags.golden`).
- **The archive drain** (`43c0e62` and on). Each run takes steps one after another for up to `DRAIN_FOR_MS`, with no
  fixed `RUN_FOR` cut-off. `RUN_FOR` stalled B0-30's first run at `archivedTo` 12,563.
- **Pacing and leaderless voters** (this round). See "Smoke 1" and "The fixes".

## Smoke 1: `repo:1048586`, 2026-11-13, with a keeper

The group is the one from B0-30's first run, at about 8.3 GB. The split started on the leader's first alarm after
the site day from `81d5c2f`.

| Step | Result |
|---|---|
| `MetaSplit` | index 28,649, 20:00:15Z |
| `MetaSeed` | 28,650 |
| Child restore | 6,375,503 rows (3.56 GB) in 803 s per member |
| `MetaDropped` | 29,013, 20:22:29Z |
| Slim pass | done 22:56Z |
| Parent member SQLite | 8.31 GB before, 3.04 GB after |
| Child member SQLite | 4.25 GB |
| Sampled comment bodies after vs before | 200 of 200 equal, 126 of them filled from the child |
| `SQLITE_FULL` | 0 |
| Archive | `archivedTo` 12,817 to 16,243, promoted to 28,429; log held at 2 GiB |
| Plants | 15 of 15 caught (`tests/am1/raw/b030split-plants.log`) |

Two problems showed up, and both are product bugs:

1. **Back-to-back slim entries tripped `STEP_DOWN`.** `MetaSlim` went out every 0.45 s. Follower `/raft` calls then
   took 400 to 550 ms each with 100 to 200 ms of CPU, so other calls queued behind them for up to about 3 s. Leader
   alarms took 3.4 to 7.5 s of CPU per 20 s slim run. Workers Logs, 20:20 to 22:57Z on sites a to c, show 110
   `StepDown`s: `CommitErrors` 104 (91 with `to: None`), `LoopLag` 5, and `Confirm` 1.
2. **The group sat leaderless.** A `STEP_DOWN` with no qualifying target holds (`to: None`), and F67's check-quorum
   then ends the leadership. Campaigns start only from requests. B0-30's load writes through the leader with
   `/bench_seed`, with no edge, so nothing campaigned, for up to 39 minutes. A keeper
   (`tests/am1/b030split/keeper.mjs`) campaigned on the home member after 30 s leaderless, 12 times. The keeper is a
   substitution, and it is out of the rerun.

## The fixes

- **Pacing** (`workers/site/src/host.rs`, `paced`). After each of its own background entries, a leader waits twice
  as long as that entry took to commit, up to 10 s. That covers the split's `MetaSlim` and the archive drain's
  `Promoted` and `Archived`. The wait is keyed on the entry's own commit time, not on shard or bucket time.
- **Leaderless voters** (`Host::keep_led`). A voter acts when two things hold: its group has leader-only work due,
  and it has seen no leader for 2 x `ELECTION_AFTER` (20 s from its last renewed promise). The work is a split step
  (`split::needs_leader`) or an archive run it led that stopped with work left in the last 10 minutes
  (`archive::leader_due`). If it names no leader, it campaigns, as one `suspect` report would make it. If it still
  names one, it first sends that leader a read index. A dead or deposed leader is then forgotten, and the member
  campaigns (F67's read-index rule). A live idle leader answers, and nothing happens. The member acts again at most
  every `ELECTION_AFTER`. This is site code only. The bound is the design's: a voter that has seen no leader for
  20 s ("Downtime", Gray region; Shape, `suspect`).
- Design: `design/DESIGN.md`, Shape, `suspect` and the split paragraph. Decision: `decisions/DECISIONS.md`, "A
  leader paces its own background writes, and a leaderless voter with such work due acts after 20 s".

Tests: `host::tests::a_leaderless_voter_with_work_due_acts_after_the_bound_only`,
`host::tests::paced_writes_wait_twice_their_commit_up_to_the_cap`, and
`split::tests::a_split_needs_its_leader_only_while_a_step_is_due`. Plants 15–24 in `tests/am1/b030split/plant.py`
cover these rules: the bound counted from each check, from the first check rather than the lapse, or not at all; acting
again on every check; acting while a promise runs; campaigning past a named leader; acting with no work due; pacing
by 1x; pacing uncapped; a child that needs a leader; and a finished pass that needs one.

## Leaked containers

From Nov 12 00:00Z to Nov 13 16:45Z, `graft-m1` billed $171.42 of containers for 73 idle standard-2 shards.

- **Cause** [verified from Workers Logs and git]. Before this round, the site script on `graft-m1` was site day 8
  (`8753c0a`). It lacks main's `3eb8315` (Nov 11), which makes a warm start no stopped shard and not count as a
  request. 8753c0a's `prsync` warm posted `/catchup` without `warm=1`. So each new leadership's warm counted as a
  client request and moved the shard's `WARM_FOR` (24 h) out again. On an idle network that happened about every
  9 h, from fold-audit wakes.
- **Not a product bug.** The idle stop worked as built. Nothing called teardown at the end of the run, and the stale
  release kept renewing. After the `81d5c2f` site day, 269 of 269 catch-ups since 20:30Z carried `warm=1`.
- **The check.** `tests/am1/b030split/shards.py` lists every running shard container of the install, with its
  instance type and start time. With `--keep NET,..` it accepts those networks. It exits 1 if any other shard is
  running, and with `--teardown` it tears those down first. Every run here ends with it. At 00:22Z on Nov 14 the only
  running shards were net 1048586's three standard-4s. They were torn down, and the check then listed none.

## Smoke 2: split and slim with no keeper, `repo:1048587`, 2026-11-14

`m3org/b030-s2` was seeded to 421 MB of member SQLite: 140,000 comment rows of about 1.8 KB. Each member's `SPLIT_AT`
was set just under that with `/bench_seed {"splitAt"}`. Driver: `tests/am1/b030split/smoke2.mjs`, with no keeper.
Raw data is in `tests/am1/raw/b030split-smoke2.{log,jsonl}`.

| Check | Result |
|---|---|
| Leader elected with no request, after `splitAt` on a leaderless group | 1 s |
| `MetaSeed`, child restore | index 6; 140,000 rows (256 MB) in about 160 s per member |
| Parent stall before `MetaDropped` | No alarm after 05:36:42Z. Fixed by `12f53da` (a split under way re-arms every 60 s); its site day landed at 05:55Z |
| Leaderless voters (`keep led`) | Followers probed the named leader every 20 s, and each probe was answered (`ReadId`), so none deposed it. At 05:59:14Z site b campaigned and led within 5 s. That was the only gap: 5 s |
| `MetaDropped` | index 14, 05:59:16Z |
| Leader cut for 15 s with `/sever`, at 06:02:25Z | Site b's voter campaigned at 06:02:41Z, and the group led again by 06:02:42Z. The driver's 5 s poll saw no leaderless sample |
| Slim pass | 05:59:16 to 06:08:33Z |
| Parent member SQLite | 421 MB before; 9.5 / 10.2 / 10.2 MB after |
| Sampled comment bodies after vs before | 100 of 100 equal |
| `SQLITE_FULL` | 0 |
| `StepDown` while slimming | **11**, all `CommitErrors`, 10 with `to: None` (a 1, b 4, c 3, a 3 by leader) |
| Cost, 05:28 to 06:15Z | $3.68 DO and Workers, $0.22 containers |

The leaderless fix held: the longest gap was 5 s, against 39 minutes in smoke 1. Pacing alone did not stop the step-downs.
Each 512-row `MetaSlim` rewrote about 0.9 MB of rows. Workers Logs from 06:04 to 06:05Z on sites b and c show 44
follower `/raft` calls: p50 0.65 s, p90 1.8 s, max 2.8 s, and 14 over `CALL_DEADLINE` (1 s). From 05:56 to 05:57Z, before
the slim, nine calls ran at p90 0.72 s with none over 1 s. Pacing spaces the entries out, but a single entry's apply still
holds a follower's call past the deadline, and missed deadlines on both followers are what `CommitErrors` counts.

**Fix** (`4cc1eae`): a `MetaSlim` entry stops at the row that brings what it has looked at to `SLIM_BYTES` (128 KiB),
and the next entry starts after that row (`decisions/DECISIONS.md`, "One `MetaSlim` entry looks at no more than 128
KiB of rows"). Test: `a_slim_entry_stops_at_its_bytes_and_the_next_takes_the_rest`. Plants 25 and 26 are both caught.

## Smoke 3: split and slim at `SLIM_BYTES`, no keeper, `repo:1048589`, 2026-11-14

`m3org/b030-s4` was seeded to 211.8 MB: 70,000 comment rows of about 1.8 KB, half of smoke 2's. It used the same driver
as smoke 2, with no keeper, on the `409a6ca` site day (`SLIM_BYTES` in). Raw data is in
`tests/am1/raw/b030split-smoke3.{log,jsonl}`.

The first attempt, on `m3org/b030-s3` (`repo:1048588`), seeded nothing. `seed.mjs` resumed from smoke 2's finished
files, which had one name for every repository, and the run split a 1.4 MB group. That log is kept as
`b030split-smoke3-unseeded.log`. Two seed fixes followed: resume files per repository (`225f703`), and abandoning a
snapshot that began before the seed's hold, which refused site 1's seed for 18 minutes (`d12e685`).

| Check | Result |
|---|---|
| Leader elected with no request, after `splitAt` | 1 s, site 1 |
| `MetaSeed` | index 9 (seed at 5), 08:13:28Z |
| Child restore | 70,000 rows (128 MB) per member in 16 to 61 s; last member 08:16:36Z |
| Parent stall before `MetaDropped` | **21 min**: no alarm ran the parent's split check after 08:16Z. My `/bench_seed` call woke it at 08:35Z, and `MetaDropped` committed at index 12 |
| Slim pass | 08:35:36 to 08:47:11Z |
| Leader cut for 15 s, at 08:38:40Z (site 0, after a hand-off) | A new leader on site 1 within 5 s; the longest gap was 5 s |
| Parent member SQLite | 211.8 MB before, 5.75 MB after on all three members |
| Sampled comment bodies after vs before | 100 of 100 equal |
| `SQLITE_FULL` | 0 |
| Cost, 07:31 to 08:50Z (both attempts and the site day's idle) | $1.78 DO and Workers, $0.40 containers |

**StepDowns** over the whole smoke window (08:12 to 08:48Z, sites a to c):

| Phase | Count | Kind and target |
|---|---|---|
| Slim pass (08:35:36 to 08:47:11Z) | 1 | `CommitErrors`, `to: None`, at 08:38:46Z on site a, with 3 of 23 calls failed to each follower. It came 6 s into the 15 s `/sever` of that leader, so it is the cut's expected result, not the slim's |
| The rest (seed, restore, the idle wait) | 0 | |

Smoke 2 had 11 `CommitErrors` StepDowns while slimming, 10 with `to: None`, and none caused by the cut.

Follower `/raft` call times while slimming (Workers Logs), by 60 s sample:

| Window | Calls | p50 | p90 | Over 1 s |
|---|---|---|---|---|
| 08:36Z, sites b and c | 194 | 0.48 s | 1.21 s | 22 |
| 08:42Z, sites a and c | 96 | 0.48 s | 0.58 s | 5 |
| 08:45Z, sites a and c | 113 | 0.71 s | 3.16 s | 55 |
| 08:30Z, sites a and c, before the slim (control) | 119 | 3.10 s | 3.17 s | 80 |

Sites a and c ran slow calls before the slim started, about 3.1 s each. That was the stray campaign below, and the
idle period showed it most. So the call times are not clean evidence for `SLIM_BYTES` alone. The StepDown count is the
measure here, and the slim tripped none.

Two problems:

1. **The parent stalled 21 minutes.** `12f53da` arms the next check from the background task after the run ends. In
   this window no alarm ran after 08:16Z [inference: the arm from a task that outlived its invocation was lost, the
   case main's `05cf1647` describes for `arm_in`]. Fix (`7c495d9`): a split under way arms its next check inside the
   alarm invocation itself, which sets it again before returning. It ships in the rerun's site day.
2. **A voter campaigned every 10 s without end.** On `repo:1048588`, site b's member is at applied 1 while its
   leader is at 14. It never learned the split, but kept its test `splitAt`, so it saw split work due, and campaigned
   354 times, each a `/raft` call to the other two members. I cleared `splitAt` on `repo:1048587` and `repo:1048588`.
   `keep_led` should not campaign at a group whose leader answers other members' appends. That member is a lagging
   follower, which its own catch-up should fix. This case only arises on a test install with `splitAt` set below the
   size [inference], and B0-30's rerun sets none.


## B0-30 rerun

`repo:1048590` (m3org/b0-30-r2) on graft-m1, site day d6d4c22 (landed 09:46Z), standard-4 shards, no keeper and no
keepHome (`B030_HOME=none`). Drivers: `tests/am1/b030split/rerun.sh` (seed, load, monitor, cross, watch).

- **Cap.** STOP_USD started at $175 under a $250 thread cap. On 2026-11-14 at about 13:40Z the user raised the cap
  to $260, and STOP_USD to $185 (relayed by T-01a17bda). `tests/am1/raw/b030-stop-usd` holds 185, which
  `monitor.mjs` reads on its next row.
- **Stopped at simulated day 1.82 (12:49:42Z).** The watcher's 5% rule tripped: log 2,257,289,087 B, more than 5% over
  `LOG_MAX_BYTES`. The drain had zero margin. One `Promoted` of about 25 entries then five `Archived` of about 5
  entries each ran every 13 to 15 s, about 134 entries a minute archived against 132 to 134 applied. Archiving a due
  range came before promoting ahead, so the log above the last promotion grew from 0.29 to 2.07 GB. At 12:49:02Z the
  leader on site a stepped down (`LoopLag`, to site 1). The new leader's first promotion took 61.7 s: its call to site
  0's shard (`/promote?wait=60&next=1`) waited 59.99 s on a catch-up that had not finished before it tried the next
  site. The log was back under the cap by 12:54Z. At 13:16Z: `SQLITE_FULL` 0, bodies 100/100, no leaderless gap,
  db 6.53 to 6.54 GB.
- **The tree changes at simulated day 1.82.** The operator chose to fix and resume, not to resume as is (a drain at
  zero margin fails the row by design) or reseed. The fix is a product change with its own tests and plants, shipped
  through one site day from b030-split plus main (which adds own-bucket-first, decode-once and leader-clock-check).
  The load resumes from `tests/am1/raw/b030-state.json`.
- **First resume, 17:23:35Z to 17:30:32Z (simulated day 1.82 to 1.905), site day 2e34351 (DONE 16:57:04Z).** The 2.07 GB
  backlog drained in about 2 minutes: each step archived 8 objects of about 44 entries, and each promotion took 50
  entries (16 MB) in 3.6 to 4.8 s. After the backlog every step carried one object. On site 0, from 17:26:17Z to
  17:29:01Z: 125 entries a minute archived and 77 promoted, against 129 applied, with the unpromoted tail rising from
  57 MB to 108 MB, under the 1.5x rule. I stopped the load (`b030-STOP`).
  The cause is a bug in the first fix: only a step's first piece was cut against the cap less the slack. Bodies were
  100/100, `SQLITE_FULL` 0, and there was no leaderless gap and no StepDown.
  The second fix cuts every piece against the step's own target, and the kept test now sweeps 0.5 to 2 times the row's
  rate, checking that promotion keeps pace at each rate (`decisions/DECISIONS.md`, the drain entry's correction).
  The same site day also lacked `keep_led`'s `heard` guard (`if heard || !needs`). A plant's mutation of `host.rs`
  was in the worktree when 3117f62 was committed, so a voter that still held a promise could act on leader-only work.
  No StepDown or keep-led line appeared on any site from 16:57Z to 17:40Z. The guard is back in the next commit.
- **Second resume, 18:32:49Z to 18:40:53Z (simulated day 1.905 to 2.005), site day 09fdeaa (DONE 18:27:26Z).** Each step
  now archives 8 objects (41 to 46 entries, 5 to 9 s apart), and each promotion takes 39 to 60 entries in 3.5 to 6.5 s.
  Leader readings, site 0, 18:34:42Z to 18:40:06Z (5.39 min):
  - applied 113 a minute, archived 148 a minute (1.31x), promoted 111 a minute;
  - unpromoted tail 64 to 100 MB, back to 66 MB, around `LOG_MAX_BYTES / PROMOTE_AHEAD` (67 MB);
  - log 2.155 to 2.201 GB on the watcher's measure, which never fell to the cap less the slack and never rose. The cap
    is 2.147 GB, so the log stayed over it; the 5% line is 2.255 GB.

  The rule asks for 1.5x, so this stops as the operator set (no third hosted fix). The log line shows two pauses of 64
  and 66 s, each right after a promotion. Over the other 276 s the drain archived 1,020 entries, 222 a minute, 1.96x
  applied.
  - Checked from the leader readings and the step log, with no hosted spend. The pauses are `SUM_EVERY_MS` (60 s), but
    the two measures do not differ. At the readings, the rows members held between `truncatedTo` and `archivedTo` were 1
    to 33 (0.3 to 10.6 MB). At the two stalls the step's own total, reconstructed from the reading before it and the
    steps since, was about 5 MB and 8 MB under the cap, and the unpromoted tail was at or under `cap / PROMOTE_AHEAD`.
    So the step's `Idle` was right by its own rule. The mechanism is that `ARCHIVE_SLACK` applies per step, not per
    run. A step that starts over the cap targets the cap less the slack, but it archives at most 8 objects (about
    17 MB). The next step to start under the cap targets the cap, finds nothing due, and ends the run. The sum then
    waits 60 s, in which about 120 entries (38 to 40 MB) arrive. So the log swings between about 8 MB under the cap
    and 40 MB over it, and never reaches the slack. The earlier note that the watcher read 40 MB higher at the stalls
    was wrong: the watcher's later readings include that minute's growth.
  Bodies 100/100, `SQLITE_FULL` 0, no leaderless gap, no StepDown from 18:27Z.

### Third drain fix: a hysteresis band per run

The operator approved hysteresis (2026-11-14, relayed by T-01a17bda); this is the last hosted fix: if it misses, the
load stops and is reported. A drain run starts when the stored log passes the trigger, `LOG_MAX_BYTES` less
`LOG_MAX_BYTES / ARCHIVE_SLACK / 2`, and archives down to the floor, the cap less `LOG_MAX_BYTES / ARCHIVE_SLACK`. The
run holds the floor as its target across steps, and across the `DRAIN_FOR_MS` continuations, until one step archives
all the way to it (`log_archive::Run`). Promoting ahead goes first only until a run passes its trigger.

- **Sizing.** `ARCHIVE_SLACK` 16 to 8: trigger 2.013 GB, floor 1.879 GB, a 134 MB half-band. One `SUM_EVERY_MS` plus
  `BUSY_MS` at twice the row's log rate (`DESIGN_LOG_BYTES_PER_S`, 1.2 MB/s: 113 entries a minute of about 320 KB,
  doubled) is 72 MB. The operator's estimate was about 80 MB; both fit. `workers/site/src/archive.rs` asserts it at
  compile time.
- **Kept test.** `the_log_stays_under_its_cap_through_the_sum_wait` (`crates/git/tests/log_archive.rs`) starts just
  under the trigger with the log promoted, then feeds input through the sum wait at 0.5, 1, 1.5 and 2 times the row's
  rate. It checks that the log never passes `LOG_MAX_BYTES` and that every run over the trigger reaches the floor
  before `Idle`. Peaks: 616, 618, 624 and 631 KB against a 653 KB scaled cap (trigger 612, floor 571).
  - A version that promoted ahead first until the log passed the cap reached 653,086 B at 2x, and failed (plant 40).
  - On e9418153's rule the log passes the cap (plant 38).

## Cost

| Window | DO and Workers | Containers |
|---|---|---|
| Nov 12 00:00Z to Nov 13 16:45Z (the leak) | $5.74 | $171.42 |
| Nov 13 19:45Z to 23:03Z (smoke 1, this thread) | $58.61 | $3.31 |
| Nov 13 23:03Z to Nov 14 00:20Z (idle split group) | $0.04 | $0.51 |
