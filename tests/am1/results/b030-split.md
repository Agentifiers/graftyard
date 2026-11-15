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

- **Third resume, from 23:03:27Z (simulated day 2.005), site day 78cf4070 (DONE 23:01:18Z).** The tree changes a
  third time, at day 2.005, for the hysteresis band. The resume started with the log at 2.077 GB, over the trigger and
  under the cap. Leader readings on site 0, 23:05:42Z to 23:22:57Z (20 readings, about 56 s apart, four drain cycles):
  - log 1.884 to 2.026 GB at every reading, under `LOG_MAX_BYTES` (2.147 GB). Each cycle archives from about 2.01 to
    1.89 GB (the floor is 1.879 GB), idles while it promotes ahead, and starts again past the trigger (2.013 GB);
  - applied 112.7 a minute, archived 139.4 a minute and promoted 135.5 a minute over the window. The 1.5x rule did not
    apply, because the log was never over its cap;
  - unpromoted tail 61 to 218 MB. It rises during each archive run, when archiving goes first, and drops to 61 to
    75 MB after the promotions that follow. It does not grow from cycle to cycle;
  - archive steps 41 to 46 entries, 7 to 10 s apart (Workers Logs, site a);
  - `SQLITE_FULL` 0, no StepDown since 12:49:02Z, no leaderless gap, no watcher alert.
  - Through 00:16Z (simulated day 2.89): log at or under 2.026 GB at all 22 readings. Three StepDowns on site a's
    member, all in the rest phase: LoopLag to Some(1) at 23:43:11Z, then CommitErrors (None) at 23:55:29Z and Confirm
    (None, p99 2,025 ms, 6 of 28 slow) at 23:55:44Z. The watcher saw no leaderless gap (longest 0 s at its 5 s poll),
    and site 0 leads again. A fourth, on site b at 00:11:28Z, was smoke 1's member during its `/retire`, not this
    group (matched by DO id).
  - 03:29:05Z (simulated day 5.34): LoopLag to Some(1) on site a's member, rest phase. Leadership went to site 2 at
    term 16, and the watcher saw no leaderless gap (0 s).
  - 03:38:58Z to 03:44:58Z (simulated day 5.4 to 5.5): seven CommitErrors StepDowns on site a's member, all `to: None`.
    These are holds: no follower qualified, so leadership did not move, and the watcher saw no gap. Six fell inside one
    archive run (03:42:43Z to 03:45:27Z, 22 `Archived` entries 7 to 12 s apart). Followers' `/raft` calls over
    03:41Z to 03:46Z ran p50 617 ms (max 3.1 s) on site b and p50 3.1 s (max 3.2 s) on site c, all ok. The CommitErrors
    rule counts calls past `CALL_DEADLINE`: 3 to 9 of 15 to 23 per follower. Earlier archive runs (`ARCHIVE_SLACK` 16,
    shorter runs) did not trip this way. The floor-to-trigger band makes each run about 130 MB, so the archive paces
    for 2 to 3 minutes at a time.
  - The operator ruled (2026-11-15) that B0-30 has no leader-change line: it passes on member SQLite under
    `CHILD_GROUP_AT` with zero `SQLITE_FULL`, archived entries readable for `RestoredTo`, and the child group taking
    the tier past the line. The holds and the LoopLag change do not count against it. StepDowns are still recorded
    by phase and target. If the holds keep lining up with long archive runs, an OPEN-QUESTIONS row is proposed at
    checkpoint (b).
  - `tests/am1/b030split/holds.py FROM TO` lines them up from Workers Logs, on the group's own objects only: each
    archive run (consecutive `Archived` lines from one site under 30 s apart), the StepDowns, and each follower's
    `/raft` count, p50, max and calls past 3,000 ms during the run and the same length before it. For 03:00Z to
    03:56Z, eleven runs of 67 to 211 s and 10 to 20 entries:
    - The seven holds all fell in two runs on site a: 03:36:54Z to 03:39:12Z (138 s, 1 hold) and 03:42:43Z to
      03:45:27Z (164 s, the longest run on site a, 6 holds). The 03:29:05Z LoopLag fell at the end of the 03:26:40Z
      run.
    - Runs with no StepDown had follower p50 321 to 428 ms, max 828 to 2,630 ms, and none past 3,000 ms.
    - In the 03:42:43Z run, the followers' members logged almost nothing (site b none, site c 10 fetches), so there
      are no follower `/raft` times for it. The 3.1 s calls seen earlier on site c were other graft-m1 objects, not
      this group's member (matched by DO id). Whether follower calls ran past `CALL_DEADLINE` in that run is not
      known.
  - 05:31:25Z to 05:33:31Z (simulated day 6.85): eight more CommitErrors holds on site a, all `to: None`. Five fell
    inside the archive run 05:30:31Z to 05:32:51Z (140 s, 17 entries) and three in the 40 s after it. Each counted 3
    to 7 of 15 to 23 calls per follower past `CALL_DEADLINE`. As at 03:42Z, the followers' members logged almost
    nothing in that window (site b 4 alarms, site c 24), so there are no follower `/raft` wall times for it. Every
    run between the two clusters (16 runs, 49 to 150 s) had no StepDown, with follower p50 337 to 420 ms and max
    under 2.7 s.
  - The load's own pushes show the slowdown in both clusters (`tests/am1/raw/b030-load.jsonl`, per minute):
    - 03:43Z and 03:44Z: 69 and 74 pushes, p50 6.7 and 6.3 s, max 11.9 and 12.5 s;
    - 05:32Z and 05:33Z: 77 and 74 pushes, p50 5.9 and 6.4 s, max 10.1 s;
    - on either side: 88 to 112 pushes, p50 4.1 to 5.2 s.
    Every push was acknowledged and `batchFail` stayed 0.
- **Correction: `CALL_DEADLINE` is 1 s.** `raft::CALL_DEADLINE` is 1 s (`crates/raft/src/lib.rs`). `holds.py` counted
  follower calls past 3,000 ms, so every "none past `CALL_DEADLINE`" above was judged at the wrong line. From 16:25Z
  it counts past 1,000 ms (`CALL_DEADLINE_MS`). At 1 s, quiet runs at 08:00Z to 08:40Z had 0 to 4 calls past it per
  follower, and runs at 15:35Z to 16:17Z had 5 to 19.

### Load stopped at simulated day 14.92: the push path slowed as the repository grew

- **What happened.** From about 10Z the load fell behind its schedule:
  - Pushes a minute (hourly means): 96 at 07Z to 09Z, then 90 (10Z), 87 (11Z), 82 (12Z), 78 (13Z), 73 (14Z) and
    66 (15Z). The schedule asks for 96 (1.6 a second).
  - The load's own push p50 rose from 4.3 to 4.9 s (07Z to 10Z) to 6.9 to 7.2 s (14Z to 15Z). With 8 pushes in flight,
    that caps throughput near 8 / p50.
  - The pushes owed (`pushDue`) were 37 to 52 until 09Z. They reached 6,604 at the stop.
  - Every push sent was acknowledged: 40 `pushFails` over the whole run, `batchFail` 0.
- **StepDowns rose with it.** Per 20-minute window: 2 (13:12Z), 4 (14:12Z), 3 (14:52Z), 7 (15:12Z), 13 (15:32Z) and
  9 (15:32Z to 16:17Z). The term went from 24 to 32 between 14:55Z and 15:33Z. All were rest phase, and the longest
  leaderless gap was 6 s (13:05:30Z and 15:24:55Z). Totals over the rerun: CommitErrors 43 holds and 4 handovers,
  LoopLag 3 holds and 6 handovers, Confirm 1 hold (`tests/am1/raw/b030r2-stepdowns.txt`).
- **The stop (16:16:23Z, day 14.916).** I wrote `b030-STOP` on the operator's stop rule, before I had computed the
  projection the rule needs. Computed after the stop (16:20Z), on the run-only meter ($58.53 at the stop, $2.15 an
  hour) plus the split's $58:
  - throughput flat at 15Z's 66 a minute: about 43,500 owed at day 30, a drain of about 11 h, total about $183;
  - throughput still falling as it did from 07Z to 15Z: about 82,000 owed, a drain of about 69 h, total about $307.

  The first case is under both stop lines and the second is over both. The trend through 16Z (70 a minute) does not
  settle which. So the load is paused, not ended: `b030-load.done` was renamed to
  `tests/am1/raw/b030r2-load-stopped-at-1616Z`, and the state resumes from `b030-state.json`.
- **The stop was premature; resumed at 16:36:01Z.** The operator ruled (2026-11-15): resume, record the growth as an
  open question (G10), and change nothing hosted mid-run. The pause ran 16:16:23Z to 16:36:01Z at day 14.916. No
  pushes or rows were sent and no simulated time passed (`load.mjs` advances active time only while it runs). The
  watcher had ended on the transient `b030-load.done` at 16:16:33Z and was restarted with the load. The drain service
  (`b030-drain`) waits for `b030-load.done`.
- **The stop rule now runs on data.** `tests/am1/b030split/project.py` projects each hour from the last 3 hours of
  minute rows: the push rate, the owed pushes at day 30 if that rate holds, the drain's wall time at that rate, and the
  total on the run-only meter plus $58 for the split. It stops when the total passes STOP_USD ($185) or the drain
  passes 24 h. At the resume (16:37Z): 70.6 pushes a minute, about 37,200 owed at day 30, an 8.8 h drain, a total of
  $172.2 at $1.92 an hour. Over 6 hours: 77.3 a minute, about 29,200 owed, 6.3 h, $170.7.
- **The drain.** `tests/am1/b030/drain.mjs` (`rerun.sh drain`) starts once `load.mjs` writes `b030-load.done` at
  day 30. It pushes every owed push at the same concurrency (8 in flight, one per branch), with load.mjs's pack shape
  and state, sending each one again up to 3 times. It stops on `b030-STOP` and resumes from the state. At the end it
  writes `b030-drain.done` (owed, carried, lost, wall time) and `b030-run.done`; the watcher now ends on
  `b030-run.done`. Tested locally against a fake edge that indexes every pack with `git index-pack --strict` and
  checks each push's parent is its branch's last tip. It answers 503 to one push in 7 and a stale `ng` once:
  - 37 owed: 37 carried, 0 lost, 7 resent, 0 wrong parents;
  - 400 owed, stopped by a STOP file after 96 and then resumed: 400 carried in total, 0 lost, 0 wrong parents,
    `pushDue` 0.

#### Push latency: which phase grew

From Workers Logs only, with no hosted change. Each hour's median is from a 3-minute sample at :20
(`.amp/in/b030diag/timeline.py`), on site a's leader, at a flat ~96 then falling push rate:

| Hour (Z) | Edge receive-pack | Shard `/validate` | Shard `/merge` (PrSync) | Member `/commit` | Follower `/raft` b / c |
|---|---|---|---|---|---|
| 03 | 4,439 ms | 1,412 ms | 1,475 ms | 1,102 ms | 412 / 413 ms |
| 07 | 4,185 | 1,869 | 2,002 | 974 | 410 / 414 |
| 09 | 4,090 | 1,994 | 2,086 | 892 | 421 / 441 |
| 11 | 5,096 | 2,609 | 2,771 | 978 | 429 / 432 |
| 13 | 6,105 | 2,978 | 3,426 | 1,060 | 553 / 522 |
| 15 | 6,616 | 3,539 | 3,919 | 1,066 | 495 / 516 |

- **The commit round did not grow.** `/commit` stayed at 0.9 to 1.1 s and the follower calls at 0.4 to 0.55 s.
  Apply is inside the commit round.
- **The git cache shard grew.** `/validate` went from 1.4 s to 3.5 s (2.5x), and the PrSync test merges from
  1.5 s to 3.9 s (2.7x). The wall time of a validate is all in the container: the shard isolate's CPU is 8 to 11 ms.
  Other shard measures grew the same way:
  - catch-up ingest per entry, 356 ms (03Z) to 601 ms (15Z);
  - `Promoted`, p50 5.8 s (05Z) to 10.1 s (15:40Z);
  - PrSync's first attempt, handled in 191 to 196 s at 05Z and 08Z, 269 to 272 s at 12Z and 345 to 351 s at 15Z.
- **The shard container is near saturated.** Site 0's shard container (standard-4) used 5,335 CPU-s in the 03Z hour,
  6,576 at 08Z and 7,062 at 13Z, about 1.5 to 2 of its 4 vCPUs on average. Its `cpu_p99`, which leaves out derived
  work (ingest, refs, repack and promotion), read 0.62 to 0.66 at 16:07Z. Sites 1 and 2's shards used 200 to 700 CPU-s
  an hour, about a tenth of site 0's.
- **What grows with the repository is the ref count.** PrSync writes `refs/pull/N/head` and `refs/pull/N/merge` for
  a pull request it handles while the pull request is open, and nothing deletes them when it closes (no delete of
  either in `crates/git/src`). Each of load.mjs's pull requests is open for 20 simulated minutes, about 67 s of wall
  time, against PrSync's 191 to 351 s first attempts, so only some get refs: 10,567 of the 90,240 opened. At 16:01Z
  the repository held 21,134 refs under `refs/pull/`, against 501 branches. The count was not read earlier in the
  run, so its growth rate is inferred, not measured. Validate's connectivity walk (`rev-list --objects --stdin
  --not --all`) and detail walk, and every merge's `--not --all`, start from every ref. A local check on git 2.56 with
  reftable and one commit per ref timed the walk for a one-commit push at 4.7 ms with 500 refs and 75 ms with 21,000
  without a commit-graph, or 2.7 to 25 ms with one. That is a 9 to 16x growth per walk, but tens of ms against a
  2 s rise, so the walk alone is not the whole cost. The rest of the growth is not measured. It may be in index-pack,
  in the walks contending for the container's CPU with the shard's own repack and promotions, or in the reftable
  stack that each ref write grows. The shard's `/status` on this deploy has no `procs`, so the split by process is
  not available.
- **The hypothesis.** Per-push work grows with the repository: confirmed for the git cache shard (validate, test
  merges, ingest and promote all grew 1.7 to 2.7x), and not for the commit round. PrSync's refs were the first
  candidate: `refs/pull/N/{head,merge}` stay after their pull request closes, so `--not --all` grows with every pull
  request PrSync handled. Measured offline below, they explain about 120 ms of a validate's 2.1 s rise at most, so
  they are not the cause.
- **The site b holds at 15:22:47Z to 15:23:52Z.** Five CommitErrors holds, outside any `Archived` run but not outside
  archive work: site b, newly leader after a LoopLag handover at 15:22:24Z, ran four `Promoted` steps from 15:22:41Z
  to 15:23:03Z (5.2 to 9.1 s each). `holds.py` counts only `Archived` lines as runs. In the same 110 s its member used
  33.3 CPU-s in 583 invocations: `/commit` 103 ms, `/read` 50 ms and `/raft` 38 ms of CPU each. Site a's member, as
  leader, used 5.1 CPU-s in 667 invocations over the same length of time, and 5.6 CPU-s at 08:20Z. Its alarms ran
  4.3 to 7.8 s of wall time each. Each hold counted 12 to 44 calls per follower with 3 to 7 failed, where a failed
  call is one past the 1 s `CALL_DEADLINE`. A new leader on a site whose member and shard are not warm pays several
  times the CPU per call while it also promotes, so its follower calls miss the deadline.
- **The ref count, measured offline.** I built the shard container from site day 78cf4070's tree and ran it locally
  on a repository of this run's shape. It has `main` and 500 branches, each tip a commit with load.mjs's 323 KB blob,
  plus one commit per pull request on its branch's chain, written with fast-import. Each pull request has
  `refs/pull/N/head` and `/merge`. Pushes, validates, ingests and ref writes went over HTTP the way the shard calls
  them. Medians of 15 calls (`/tmp/vb/bench.py`, not kept):

  | Repository | `/validate` | `/merge` | `/ingest` | `/refs` |
  |---|---|---|---|---|
  | 500 pull refs | 14 ms | 26 ms | 2 ms | 2 ms |
  | 21,000 pull refs, packs as ingested | 134 ms | 32 ms | 3 ms | 3 ms |
  | 21,000 pull refs, after repack and commit-graph | 23 ms | 25 ms | 2 ms | 2 ms |

  21,000 refs add about 120 ms to a validate before the shard's repack writes a commit-graph, and about 10 ms after.
  They add about 6 ms to a merge. The hosted rise was about 2,100 ms on validate and 2,400 ms on merge, so the ref
  count is not the main cause.
- **Lock wait.** This deploy logs no lock wait: the container's `/validate` and the shard object's route have no
  `lock_ms` or similar. A validate takes the network's lock shared. `/ingest`, `/refs` and a repack's writers take it
  alone, so a validate queues behind a catch-up's writers. The container ran at 1.5 to 2 of 4 vCPUs with `cpu_p99`
  0.62 to 0.66, not CPU-bound, so waiting is the stronger lead. It has no number on this run.
- **What GHES does on close.** GitHub keeps `refs/pull/N/head` after a pull request closes or merges, for history,
  and removes `refs/pull/N/merge`. GitHub's docs call both refs temporary and read-only and say nothing about close, so
  this is observed behaviour, not documented: GitHub community discussion #11922; mtilson's `refs/pull` gist; Fluffy
  and Flakey, "What is a GitHub Pull Request merge branch?". It is not verified on GHES 3.22. `design/SHARED.md`,
  "Pull request state never diverges from refs", names both refs as derived and says nothing about close. So deleting
  head refs on close is not an option. Deleting merge refs on close would match GitHub, but it halves the ref count at
  most.
- **What I did not check.** Whether index-pack's own time is the growing part, and how the validates' lock wait splits
  against repack. Both need main's `/procs` and a lock-wait measure on a hosted run. Main now has repack-keep
  (`0c171e6d`) and `/procs` (`0c171e6d`, `361a1dc0`). Branch `disk-validate` (`23fc1dd0`, not yet on main) validates a
  push on the shard's disk as it stands, skipping the catch-up wait. This run's deploy predates all three. Open
  question G10 (`decisions/OPEN-QUESTIONS.md`) records it.

- **The run's own meter (2026-11-15, 00:07Z).** `usage.py` counted every graft-m1 namespace, and `containers.py` every
  graft-m1 shard, so other groups on the install were charged to the rerun. Durable Object analytics split by object
  (`objectId_in` on `durableObjectsPeriodicGroups` and `durableObjectsInvocationsAdaptiveGroups`), and container usage
  by instance (`instanceId`), so nothing is subtracted:
  - `tests/am1/b030/objects.py` lists the group's Member and GitShard objects. These are every object whose Workers
    Logs name `repo:1048590`, `repo=1048590` or `1048590/`, and the child's `repo-meta:1048590` once it exists. Six so
    far: members a `6fd8c123…`, b `f13385ed…`, c `13d912fe…`, and shards a `2b6ed590…`, b `1c6a9537…`, c `59f9e8d8…`.
    `monitor.mjs` refreshes the list on every row.
  - With `B030_OBJECTS`, `usage.py` counts DO requests, rows written and active time on those objects only. Workers
    requests and CPU have no object dimension and stay install-wide, so the meter still slightly over-counts.
  - With `NETS=1048590`, `containers.py` counts only `git#<site>#net:1048590` instances.
  - Corrected spend at day 2.8 (00:07Z): $23.41 ($18.65 DO and Workers, $4.76 containers), against $48.64 on the old
    meter. From 18:41Z to 23:04Z, with the load stopped, the run's own objects cost $0.16 and its shards $1.50. The old
    meter's $23.16 for that window was smoke 1's `repo:1048586`, whose site b member (`a217c64f…`) wrote 19.5 M rows
    reinstalling a 3.16 GB tier (`install`, 3,467,920 rows through 16,259, 1,022 s), plus smoke 3's `git#0#net:1048589`
    container and other installs' objects.
  - STOP_USD stays at $185 (the user's number), now judged on this meter.
  - Projection on this meter: $23.56 at simulated day 2.83 (00:09Z). The load runs at $2.12 an hour (23:10Z to
    00:10Z: $1.69 DO and Workers, $0.43 containers; 10:30Z to 12:30Z was $1.97 an hour). The remaining 36.2 h of active
    time costs about $77, and the split about $58 (smoke 1's window, an upper bound, since that was the install-wide
    meter). Total about $158, $27 under STOP_USD.
- **Teardown of smoke 1's and smoke 3's leftovers (2026-11-15, 00:09Z to 00:12Z).** No row or thread named
  `repo:1048586` as live:
  - `tests/am1/results` names it only as B0-30's first run ("left as it is ... for the rerun to inspect or replace")
    and as smoke 1.
  - No other branch's results file names it.
  - The drivers read `tests/am1/raw/b030-repo.json` (1048590).
  - `git#0#net:1048589` (smoke 3's shard, running since 09:34Z) was torn down with `teardown.py 1048589 1048589` (200).
    `repo:1048586` had no running shard.
  - Each member of `repo-meta:1048586` and `repo:1048586` was cleared with the test route `/retire`, as
    `tests/m3/wirefix/recreate.mjs` does: storage deleted, object reset (`tests/am1/raw/b030-retire-1048586.log`).
    Before, `repo:1048586` was led on site 1 at term 379, commit 36,576. Its members held 2.77, 3.04 and 2.77 GB of
    SQLite with logs of 1.88, 2.15 and 1.88 GB, archivedTo 18,041 / 16,259 / 18,041; the metadata tier was dropped at
    the split. `repo-meta:1048586` held 4.25 GB on sites 0 and 2 (the seeded tier at 28,595) and 1.4 MB on site 1, at
    commit 9 to 11.
  - After, all six members answer "not joined". `shards.py --keep 1048590` lists only the rerun's three shards.
  - Missed at first: site b's voter was a replacement, `repo:1048586#5#f1`, not the generation-1 name that `/retire`
    reached. It stayed joined at term 379, commit 36,576 (6.20 GB, log 1.88 GB). Logging "keep led: no leader seen ...
    campaign Ok(Started)" every 10 s, it wrote no rows (active time only), so it cost almost nothing. Retired by name at
    03:34Z (2026-11-15). It answers "not joined", and its alarms stopped.
  - Its R2 archives and snapshots are left in the buckets. They are what `RestoredTo` reads, and they cost storage
    only.
  - `teardown.py --sweep` is on `lm1-rows-m3f`, not on this branch. `shards.py` (this branch's end-of-run check) did
    the same check.

## Cost

| Window | DO and Workers | Containers |
|---|---|---|
| Nov 12 00:00Z to Nov 13 16:45Z (the leak) | $5.74 | $171.42 |
| Nov 13 19:45Z to 23:03Z (smoke 1, this thread) | $58.61 | $3.31 |
| Nov 13 23:03Z to Nov 14 00:20Z (idle split group) | $0.04 | $0.51 |
