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
  open question (G14), and change nothing hosted mid-run. The pause ran 16:16:23Z to 16:36:01Z at day 14.916. No
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
  head refs on close is not an option. Deleting `refs/pull/N/merge` on close matches GitHub's observed behaviour and
  would halve the ref count at most; it is recorded as an option, unverified on GHES 3.22, not a fix to build now.
- **What I did not check.** Whether index-pack's own time is the growing part, and how the validates' lock wait splits
  against repack. With the refs measured at 120 ms at most, the lock wait (ii) is the lead. What would measure it is
  on main: disk-validate (merged as `a304d3fc`) validates a push on the shard's disk as it stands, skipping the
  pushes' catch-up wait, and `/procs` (`0c171e6d`, `361a1dc0`) shows what the container runs; main also has
  repack-keep (`0c171e6d`). This run's deploy predates all three. The next B0-30-shaped run from main would show
  whether validate time stays flat. Open question G14 (`decisions/OPEN-QUESTIONS.md`) records it.

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

### Second stop at simulated day 17.62, and site day 9

- **The stop (20:12:25Z, day 17.619).** `project.py` printed `stop: true`: total $185.6 against STOP_USD $185
  ($65.77 spent at $2.06 an hour, 60.5 pushes a minute, 49,211 owed at day 30, a 13.6 h drain). I wrote `b030-STOP`
  and the load stopped at 20:12:56Z. The drain service started on `b030-load.done`, saw `b030-STOP`, pushed none of
  the 14,072 owed and wrote `b030-run.done` 11 ms later, which ended the watcher. The three `.done` files were renamed
  `*.done-stop2012Z` so the end steps would not run. At the stop: member SQLite at most 7.30 GB, log 1.939 GB, zero
  `SQLITE_FULL`; the split had not started (SPLIT_AT is 7.52 GB).
- **The forecast was about $5 low from 16:46Z to 19:12Z.** `project.py` took its hourly cost rate from the first and
  last monitor rows of its 3-hour window. Until 19:32Z that window held the 16:20Z row, inside the pause, which billed
  $0.10 in 15 minutes, so the rate read $1.88 an hour against a running $2.05. `project.py` now counts only monitor
  intervals the load ran through (a gap of 2 minutes or more in the minute rows, or a stopped load, is a pause). At
  20:29Z the fixed rule reads $2.06 an hour where the old one read $1.99.
- **Stop line raised to $225 (user's approval, about 20:25Z on 2026-11-15, relayed by the operator).** STOP_USD in
  `tests/am1/raw/b030-stop-usd` is now 225. The 24 h drain limit is unchanged.
- **Site day 9 (announced 20:40Z, start 20:50Z).** The operator chose a site day from main and a resume over ending:
  ending would fail the split clause, and a failed 30-day row costs a full rerun. The site script is `cb24cb47`,
  b030-split (`9cb76687`) merged with main `c653084d`; main's new code is M4 (the broker's stored `Seen`, Sandbox RPC)
  and nothing in the split or archive paths. Against what this run ran since 78cf4070, the split code
  (`crates/git/src/split.rs`, `workers/site/src/split.rs`) is unchanged; the archive gains main's promote-stream (the
  pack streams from the shard to the buckets and the validate; an entry over `PROMOTE_MAX` promotes alone), its
  `View::unpromoted` counts pack bytes, not row bytes (`pack_bytes_above`; equal for this run's inline pushes), and the
  fold's `Store` gains `wrote` and `applied` hooks. The band, the run hold and promote-ahead are unchanged. New git
  image `graft-m1-site-gitshard-git@sha256:8c972c8b…94a8` (tag `b030s9`, `tests/am1/raw/b030split-image-9.log`), from
  main's `containers/git`: disk-validate (`/validate?disk=1`), repack-keep, and `/procs` on `/status`. Running shards
  are rolled to it (`--roll-shards`). Site variables unchanged. Script: `tests/am1/b030split/siteday.sh`.
- **Site day 9 done (20:50:15Z to 21:23:5xZ, exit 0; `tests/am1/raw/b030split-siteday-9.log`).** Sites a to d
  deployed in two phases, the last versions a `491c7ce5`, b `a50c6921`, c `05dbc6e9`, d `edd24be7`. The roll killed
  and caught up each shard of net 1048590 on the new image: site 0 by 21:08Z, site 1 by 21:14Z, site 2 by 21:23Z
  (`kill 200, catchup 200` each). Checks after it:
  - The group kept its state: leader site a at term 43 before and after, applied 145,845, archivedTo 140,589,
    promotedTo 145,625, log 1.953 GB, member SQLite 7.198 GB, zero `SQLITE_FULL`.
  - `/status` shows `procs` on all three shards (no git process running at 21:24Z, one pack of 57.7 to 58.0 MB each).
  - The archive's promotion still validates with `derived=1` (3 calls, p50 1,227 ms in 21:22 to 21:24Z), and
    `validate_on` catches a derived validate up first [code: `crates/git/src/shard.rs`, `validate_on`].
  - Push validates: 105 in 21:22 to 21:24Z, p50 376 ms, max 2,472 ms, against 3,539 ms at 15Z. The shard object's
    log line is its own URL, which carries no `disk` flag (the flag is on its call to the container), so that these
    ran on `disk=1` is [inference] from the drop and from `validate_on` (a pushed validate on a running container this
    isolate has caught up goes on the disk as it stands). No `missingBase` or `held=` in site a's logs in that window.
- **Resumed at 21:24:24Z, simulated day 17.627** (operator, 2026-11-15). `b030-STOP` renamed
  `tests/am1/raw/b030r2-STOP-2012Z`, the renamed `.done` files deleted, and `b030-load`, `b030-drain` and
  `b030-watch` restarted; the load continued from its state at 121,248 pushes. Stop rule: STOP_USD $225, drain 24 h.
- **The first two hours after the resume (21:24Z to 23:32Z, simulated day 17.63 to 19.23).** Hourly samples, 3
  minutes at :20 on site a's leader (`tests/am1/b030split/hourly.py`, `tests/am1/raw/b030r2-hourly.jsonl`):

  | Sample | Shard `/validate` p50 | `/merge` p50 | Pushes a minute (hour) | `/procs`, six snapshots |
  |---|---|---|---|---|
  | 15Z, before the site day | 3,539 ms | 3,919 ms | 66 | not on that image |
  | 22:00 to 22:03Z | 425 ms (217) | 1,315 ms (71) | 81 | no git process |
  | 22:20 to 22:23Z | 432 ms (180) | 1,747 ms (71) | 80 | one `index-pack`, 0.0 CPU-s |
  | 23:20 to 23:23Z | 399 ms (258) | 1,382 ms (82) | 80 | `pack-objects` 0.9 CPU-s, `index-pack` 0.1 |

  - Validate did not stay near 3.5 s, so G14 records the catch-up wait, with what the roll cleared from the
    shards' disks, as the cause; the roll changed both at once.
  - The load ran 79.3 pushes a minute over the 126 minutes, against 60.5 before the stop, with `batchFail` 0. Its own
    push p50 stayed at 4.4 to 7.5 s a minute (hour medians 5.3, 5.1 and 5.5 s). The edge's receive-pack p50 was 5.0 s
    at 21:27Z, and site a's member `/commit` p50 1.0 s. The time is now outside the shard.
  - Pushes answered HTTP 503 (git cache: no shard could check the pack, or a pack store error; the edge does not log
    which): 194, 370 and 185 in the 21Z, 22Z and 23Z hours, about 4 to 8% of pushes, against 0 to 9 an hour before the
    site day. In 23:20 to 23:23Z, all 10 edge 503s fell between 23:20:05Z and 23:20:44Z, within 3 s of each of three
    CommitErrors holds on site a (23:20:04Z, 23:20:23Z, 23:20:33Z), and every validate on the three sites answered 200.
    The load retries them, so no push is lost.
  - StepDowns rose: 115 from 21:24Z to 23:12Z and 17 more to 23:32Z, against 2 to 13 per 20 minutes before. All are
    rest-phase holds, 129 of 132 CommitErrors and 3 Confirm, 130 on site a, and 130 fell inside archive runs (131 to
    188 s each, 5 to 11 holds a run). The term went from 43 to 47, with the longest leaderless gap 6 s. Follower `/raft`
    in those runs: b p50 165 to 180 ms, max 1.1 to 1.6 s; c p50 254 to 270 ms, max 1.1 to 8.6 s; 2 to 8 calls a run
    past CALL_DEADLINE (1 s).
  - Archive: about 93 entries a minute applied, 92 archived and 92 promoted; log 1.95 to 2.03 GB, under its cap at
    every reading; member SQLite 7.17 to 7.39 GB, zero `SQLITE_FULL`.
  - Cost on the run-only meter: $66.51 at the resume, $70.94 at 23:20Z. One meter interval, 22:20 to 22:35Z, billed
    $1.03 (DO rows written 927,000 against about 400,000), and the others $0.50 to $0.51. `project.py` at 23:32Z:
    79.3 a minute, 29,867 owed at day 30, a 6.3 h drain, total $176.9 against $225.

### The split begins (2026-11-15 23:51:41Z, simulated day 19.5)

- **Start.** The watcher's first split row with a child came at 23:51:41Z, leader site a, parent SQLite 7,533,592,576 B
  (over SPLIT_AT, 7,516,192,768 B). `MetaSplit` names `repo-meta:1048590`, and `MetaSeed` names the metadata snapshot
  through entry 157,929 (`snapshot/repo:1048590/metadata/…-157929-f43e3247…`). That is days earlier than my day 23 to
  24 estimate, which took the leader's net growth from too short a window.
- **First numbers (00:10Z on 2026-11-16, `tests/am1/raw/b030r2-child.jsonl`).** The child group's three voters are at
  commit 5, leader site a, log 674 B, `childLoaded` false: each is still restoring the seed. Child SQLite 1.979 GB on
  site a, 1.840 GB on site b (`#5#f1`), 3.163 GB on site c. The parent has dropped nothing yet (`MetaDropped` waits for
  the child leader's restore): 7.423 / 7.362 / 7.422 GB, log 1.90 to 1.96 GB. `SQLITE_FULL` 0 on all six members.
- **The meter.** The child's site-a member (`8e88c6d9…`) wrote 2,121,798 rows in the 23:50Z five minutes and 525,175 in
  23:55Z, against about 43,000 for each parent member. So the 23:33 to 23:54Z monitor row billed $2.77 (2,634,148 DO
  rows written). That is the split's own cost, which `project.py` already adds as $58, and it now also lifts the
  hourly rate it extrapolates ($3.13 an hour at 00:11Z), so the projection ($196.2) counts the restore twice while it
  runs [inference].
- **The earlier $1.03 interval (22:20 to 22:35Z).** Monitor row 22:33:50Z: cost +$1.03, DO +$0.93, rows written
  +927,507 against about 395,000, requests +23,498. Five-minute rows by object put 580,755 of them on the parent's
  site-a member (`6fd8c123…`) in 22:30Z, against about 43,000 every other five minutes; not traced further, since the
  split's rows are a different object and a different cause.

- **The child's restore finished, and the parent dropped (Workers Logs, `tests/am1/raw/b030r2-child.jsonl`).** Each
  child member logged `split: child restored … 4,949,926 rows, 3,241,503,104 bytes`: site a's member at 00:14:38Z
  (1,265 s), the other two at 00:27:58Z and 00:28:20Z (1,996 s and 2,179 s). The parent's leader committed
  `MetaDropped { seed: 157929 }` at index 162,475 at 00:28:58Z, and the slim pass started on table 0 (`issue`).
  - At 00:56Z all three child members read `childLoaded` true, commit 5, log 674 B, SQLite 3,836,465,152 B each.
  - The child had no leader at 00:56Z: all three are followers at term 2. That is allowed: a child needs no leader
    (`split::needs_leader`), and a fill read campaigns. The bodies check at 00:54:55Z, the first one after
    `MetaDropped`, read 100 of 100 equal. The slim had not yet reached the seeded comments, so the fill path is not
    exercised yet.
  - Parent SQLite peaked at 7,556,329,472 B (00:28:55Z), then read 7.360 / 7.299 / 7.386 GB at 00:56Z. The slim's
    cursor moved from issue key 1,006,532 (00:33:53Z) to 1,049,274 (00:56:59Z). The parent's leader moved to site b at
    term 60.
  - `SQLITE_FULL` 0 on all six members at every reading.
- **The split's cost, measured (`tests/am1/b030split/project.py`, operator, 2026-11-16).** The restore wrote 29.7 M
  DO rows on the three child objects from 23:45Z to 00:45Z: $29.72 on usage.py with only those objects. One
  five-minute row on site c's child (`24b734c0…`, 00:10Z) wrote 5.2 M. The old rule added a flat $58 for the split
  and also took the restore into its run rate, so it counted the restore twice:
  - At 00:26:26Z it printed `stop: true`: $8.17 an hour, total $316.2. At 00:49:09Z: $12.66 an hour, total $423.6.
  - I did not stop on these, because the operator had already ordered the measurement fixed. Both lines are in
    `tests/am1/raw/b030r2-notes.txt`.
- **The fix, the same kind as the pause window.** The run rate now skips meter intervals that overlap the split: from
  the first watch row with a child to the slim pass's end.
  - The split's cost is in the meter as it is spent, so only what is left of it is added. The `splitSoFar` field reports
    the meter's rise since the last row before the split, less the run rate.
  - What is left after the restore is the slim. Its estimate, `SLIM_EST_USD` ($18.25), is smoke 1's measured slim on its
    three parent members (`61f46f63…`, `cfb22d9a…`, `ebda6705…`, 20:22Z to 22:57Z on 2026-11-13: 20.0 M rows, $20.04)
    scaled from 3.56 GB to this child's 3.24 GB. The slim's spend counts from the first meter row after `MetaDropped`.
  - If the slim passes its estimate, each reading adds one more hour at the slim's own rate and sets
    `slimOverEstimate`.
- **Before and after, on the same data (00:49:09Z):** old rule $12.66 an hour, total $423.6, stop. New rule $2.29 an
  hour, split so far $29.47, slim left $18.25, total $169.0 against $225, drain 7.6 h, no stop.
  - The total is $103.43 spent, plus $2.29 an hour over 13.6 h of active time and the 7.6 h drain, plus $18.25.
  - While the slim runs, the rate comes from the last 3 hours before the split [inference: the rate after the slim
    should be lower, since the parent is smaller].
  - Pacing makes the slim slow. Parent members wrote 72,000 to 83,000 rows per five minutes in 00:30 to 00:35Z,
    against about 42,000 before. If that holds, 4.95 M slimmed rows take about 12 h and cost about $15 [inference].
- **Holds and archive runs (`tests/am1/b030split/runs.py`, `tests/am1/raw/b030r2-runs.jsonl`).** These are archive
  runs on the parent's leader, with StepDowns from 15 s before a run to 15 s after it.

  | Window | Runs | Archived a run (mean, range) | Length (mean, range) | Archived MB, all runs | Holds in runs / outside | Holds a run (mean, max) | Pearson holds vs archived / length | Spearman holds vs archived |
  |---|---|---|---|---|---|---|---|---|
  | 18:10 to 20:12Z, before the site day | 20 (19 on a, 1 on b) | 450 (68 to 584) | 186 s (24 to 260) | 3,360 | 25 / 1 | 1.25, 3 | 0.16 / 0.15 | −0.03 |
  | 21:30 to 23:30Z, after it | 20 (all on a) | 571 (194 to 741) | 242 s (84 to 323) | 4,346 | 128 / 1 | 6.4, 11 | 0.11 / 0.11 | 0.03 |
  | 23:40 to 00:00Z, the F74 ring | 3 | 509 | 235 s | 587 | 40 / 0 | 13.3, 20 | 0.83 / 0.92 (n = 3) | 0.5 |

  - The guess that more entries a run means more holds does not hold within a window: both 20-run windows correlate
    at 0.16 and 0.11 at most.
  - Across the site day, holds a run rose about 5 times while entries a run rose 27%. Pooled over both windows the
    Pearson is 0.42, and that comes from the shift between windows, not from the size of each run.
  - Nearly every hold is inside an archive run: 153 of 155 in the two windows. So the run itself, or what changed at
    the site day, is the lead, not its size [inference].
- **Follower `/raft` legs during runs (F74's `tests/f74/calls.py`, ring 23:42 to 00:00Z on site a's leader).** Out and
  back are skew-corrected as calls.py does. Rows: `tests/am1/raw/b030r2-calls-*.tsv`.

  | Interval | Calls | Missed 1 s | Largest leg out / back / work | Leader to b: total p50/max, work p50 | Leader to c: total p50/max, work p50 |
  |---|---|---|---|---|---|
  | Run 23:44:03Z (284 s) | 3,057 | 424 | 264 / 139 / 11 | 369 / 3,204, 102 ms | 573 / 2,897, 178 ms |
  | Run 23:51:35Z (320 s) | 3,742 | 695 | 358 / 307 / 21 | 401 / 2,859, 88 ms | 585 / 3,417, 193 ms |
  | Between them, 23:48:47 to 23:51:35Z | 1,911 | 58 | 51 / 6 / 1 | 326 / 1,721, 116 ms | 480 / 2,067, 191 ms |

  - Misses are 7 to 12 times as frequent inside runs. Out is the largest leg in most of them, and follower work stays
    at 88 to 193 ms p50.
  - Caller lag at send was under 100 ms for 11,872 of 11,911 calls. So the time is in transit or in a caller-side wait
    that the loop-lag ticker does not see [inference]. That matches F74's isolate-stall question, and the rows went to
    B0-02's thread (T-01a1ccdd-804f-769d-b81a-5f937e3309df).
  - The 23:51:35Z run overlaps the split's first minutes, so it is not a clean run.
- **Proposed OPEN-QUESTIONS row (not added; for the operator):** "Does a sustained archive run slow follower commits?"
  - On B0-30's rerun, 153 of 155 CommitErrors holds fell inside archive runs, and follower `/raft` misses are 7 to 12
    times as frequent inside a run.
  - The largest leg is out, not follower work.
  - Holds a run do not track entries a run (Pearson ≤ 0.16 per window), but they rose about 5 times across site day 9.
  - Candidates, all unmeasured:
    - the leader's R2 puts and `Promoted` commits sharing its isolate with the `/raft` fetches;
    - site day 9's image or script;
    - more pushes a minute after the site day (80 against 66).
- **503s near holds (`tests/am1/b030split/near.py`, `tests/am1/raw/b030r2-near.jsonl`).** Edge receive-pack 503s per
  hour, counted within 3 s of a hold or outside every hold:

  | Hour | 200s | 503s | Within 3 s of a hold | Outside | Holds |
  |---|---|---|---|---|---|
  | 18Z | 3,565 | 4 | 0 | 4 | 15 |
  | 19Z | 3,580 | 1 | 0 | 1 | 9 |
  | 20Z (stop at 20:12Z) | 689 | 1 | 0 | 1 | 5 |
  | 21Z (resume at 21:24Z) | 2,706 | 189 | 88 | 101 | 41 |
  | 22Z | 4,795 | 373 | 188 | 185 | 64 |
  | 23Z | 4,604 | 470 | 252 | 218 | 81 |

  - About half fall within 3 s of a hold (528 of 1,032 after the resume), so the results do not say "commit-side". The
    other half have no hold near them.
  - This corrects my two-hour report: there I counted all 10 of the 23:20Z sample's 503s as near holds, and near.py
    counts 6 of 10.
  - No code change was made, as ruled.

- **The 503s: one message, the push's read gate (operator, 2026-11-16).** The load counts each failed push as
  `push http 503` and keeps no text, so I took the text another way, with no code change and nothing hosted changed.
  - **Probe.** `tests/am1/b030split/probe.mjs` (`b030probe`, 01:25 to 01:45Z, 547 calls every 2 s) posted git's
    bare-flush probe (`0000`) to `git-receive-pack` with the load's token. The edge runs the same resolve and read
    gate as for a real push, then answers 200 without reaching the group. 488 answered 200. All 59 others were 503
    with one message: `the repository's access rows did not answer; retry (repo:1048590: every member asked shed the
    read: overloaded)`. Wall time 1.7 to 4.1 s, p50 2.9 s.
  - **Push traces.** I followed each edge 503 by trace id through Workers Logs (`/tmp`-only scripts):
    - 22 of 22 between 14:00 and 20:12Z (before the site day);
    - 2 of 2 at 22:00 to 22:01Z;
    - 10 of 10 at 01:00 to 01:03Z.

    Every one had the same shape. The group's member calls were only one or two follower `/read`s, on sites b and c,
    one usually near 1 s; there was no `/validate` and no `/commit`, and the edge answered at 1.9 to 5.3 s (p50 about
    2.9 s). A 200 push in the same windows ran 8 to 15 s through `/validate`, `/commit` and `/outcome`. So the 503s,
    before and after site day 9, come from the read gate, not from the shard or the commit [inference from trace shape
    and the probe's text].
  - **The code path.** `workers/edge/src/lib.rs`, the router's read gate (`read_gate` then the `Err(e)` arm, "the
    repository's access rows did not answer").
    - `read_gate` reads the group's `access` rows with `api::core_rows`, which uses `spread_read` →
      `graftyard_lookup::read_spread_for`. That asks the followers by read-index: two followers, no learners on this
      group.
    - A follower that has not applied the leader's read index within FOLLOWER_WAIT (1 s) answers `overloaded`
      (`workers/site/src/member.rs`, `read_wait`). When both do, `spread_verdict` returns `Shed`, and the edge answers
      503 without asking the leader.
  - **Why it rose across site day 9.** Unmeasured. What I can see:
    - The edge script did not change: `graft-m1-edge` has run version `19b495a1` since 2026-11-13 19:59Z. So the
      change is on the member side.
    - The site script went from `78cf4070` to `cb24cb47`. That brought main's lease-clock (`27eece68`: lease reads
      take a round while the clock lags) and memory-budgets (`0cc44e68`, `b5f7199e`: answer and AppendEntries byte
      budgets). Either could slow a follower's read-index grant. Neither is measured.
    - The deployed edge predates lease-clock's other half, "a write retries a shed spread read" for LOOKUP_FOR
      (`crates/lookup/src/lib.rs`, `read_spread_for`). On main's edge these pushes would have waited and gone through,
      not answered 503 [inference from the code].
  - **Follower read stats now (member `/stats`, 01:49 and 02:11Z, `tests/am1/raw/b030r2-readstats.jsonl`).**
    - Site b: 10,522 follower reads, 2,394 late (23%), grant p50 539 ms, p99 985 ms.
    - Site c: 7,491 follower reads, 6,004 late (80%), grant p50 554 ms, p99 977 ms.
    - No reads shed on any member (`shedReads` 0). So `overloaded` here is the 1 s FOLLOWER_WAIT running out, not the
      READS_MAX or byte cap.
    - A grant p99 at 0.98 s against a 1 s wait means a small slowdown tips both followers over at once.
  - **Site c falls behind.** Its member was 0 to 5 entries behind the leader through 23:18Z. Since 23:33Z it has
    fallen 15, 243, 428, 582, 883, 1,125, 1,267, 1,566, 1,750, 1,869 and 2,091 behind (monitor rows to 02:09Z), applying
    66 to 88 entries a minute against the leader's 77 to 106. That is why site c's read-index reads are mostly late.
    - The lag started with the split (MetaSplit 23:51Z). Site c's child member restored the largest seed burst
      (5.2 M rows in the 00:10Z five minutes), and the slim's entries rewrite rows on every parent member. That this
      slows site c is [inference].
    - The lag does not touch the gate. Site c's SQLite is 7.26 GB, log 2.00 GB, zero `SQLITE_FULL`.
  - About half the 503s fall within 3 s of a hold (near.py). That fits the cause: a hold stalls the leader's rounds,
    so both followers miss the grant. The other half are a follower's normal late tail [inference].
  - Nothing fixed mid-run, as ruled (operator, 2026-11-16): the load retries them and loses none, and main's edge
    already retries a write's shed read within LOOKUP_FOR, so the fix ships with the next edge deploy. Site day 9
    deployed the four site scripts and kept edge `19b495a1` from 2026-11-13, so this run's edge is older than its
    members. Recorded as `decisions/OPEN-QUESTIONS.md` G16. Hourly 503s from the load's tally: 1 to 9 an hour from
    14Z to 20Z, then 194, 370, 472 and 754 in the 21Z to 00Z hours.
- **Site c's parent member, watched (operator, 2026-11-16).** Monitor rows, 15 minutes apart:

  | Time | Behind the leader | Its apply rate | Leader's | Above the leader's truncation | InstallSnapshot |
  |---|---|---|---|---|---|
  | 23:18Z | 2 | 98.4 a min | 98.5 | 5,124 | 0 |
  | 00:24Z | 582 | 66.3 | 76.5 | 4,428 | 0 |
  | 01:24Z | 1,566 | 85.9 | 105.8 | 5,016 | 0 |
  | 02:24Z | 2,326 | 85.3 | 101.0 | 4,721 | 0 |

  - Its place in the log: it needs an `InstallSnapshot` once its next entry falls below the leader's truncation point,
    which follows `archivedTo`. The log the leader keeps is about 7,000 entries (its 2 GiB cap), so the margin is about
    7,000 less the lag.
  - The lag grew 12.7 entries a minute over the last hour, but the margin fell only about 5 a minute (5,016 to 4,721),
    and about 2 a minute since 23:18Z (5,124), because the kept log moves with the archive runs. At 5 a minute the
    margin lasts about 16 h, past day 30 (about 11.5 h of active time and an 11 h drain left) only if the drain does
    not speed it up [inference from a 1 h and a 3 h trend].
  - The member's own `/stats` at 02:30Z: installs 0, restores 1 (its boot).
 (`tests/am1/b030split/fill.mjs`, 01:54:57 to 01:55:02Z,
  `tests/am1/raw/b030r2-fill.jsonl`).** The slim had moved into the comment table, cursor key 2^40 + 21,024. So two of
  the watcher's 100 sampled seeded comments (keys 2^40 and 2^40 + 13,400) were already slimmed on the parent. I read
  each through the parent's leader (site b) twice, and read two unslimmed sample rows as a control.
  - **Before:** the child's three members were followers at term 2, no leader, commit 5.
  - **Slimmed rows:** both read back equal to the pre-split baseline.
    - Key 2^40: 714 ms the first time, 195 ms the second (the isolate's child-row cache).
    - Key 2^40 + 13,400: 511 ms, then 531 ms.
    - Control reads of unslimmed rows: 422 ms (one other answered `notLeader`, as the parent's leadership moved then).
  - **The fill reached the child.** Workers Logs show child site c's member answer `/read` (318 ms) and child site a's
    (441 ms) in the first read's window, after correcting for the orb clock running about 6 s behind Cloudflare's.
  - **The fill campaigned.** After the reads, child site a led at commit 7 (from 5), with b and c following it. So a
    fill read on a leaderless child started an election, as designed (`split::needs_leader` is false for a child; a
    fill read campaigns).
  - Follower read-index reads of those rows answered `notLeader`: a follower does not serve point row reads
    (`follower_serves`), so the edge goes to the leader for them.
  - **Owed before verify:** run `fill.mjs 10` again once the slim has passed more of the 100 sample keys, so that ten
    or more slimmed rows are read, not two.
- **G15 added to `decisions/OPEN-QUESTIONS.md`** after a merge of origin/main (`a45a316f`), which took G10. The table
  now reads Git and storage 14, Total 146, recounted from the rows. (Numbered G14 then; the merge of origin/main
  `ba13a063`, whose G11 is B0-31's, moved this branch's G11 to G15 up by one, to G12 to G16.)
- **Orb restart (2026-11-16, about 04:11 to 04:24Z).** The orb that drives the run restarted: the load's last minute
  row was 04:11:14Z at simulated day 22.712, and every service was down at 04:21Z. The tails of `b030-load.jsonl` and
  `b030-cross.jsonl` held NUL bytes from the cut (156 and 423), which I stripped; every line parses. The state file was
  intact. All six services restarted at 04:24Z, and the load resumed at day 22.721 with 148,045 pushes. `load.mjs`
  advances simulated time only while it runs (at most 5 s a tick), so the outage cost no simulated time; the group ran
  on, archiving, with no pushes for 13 minutes. Nothing hosted changed. The watch readings moved from `/tmp` into
  `tests/am1/b030split/read.mjs`.
- **Site c's margin, tracked (operator, 2026-11-16).** `tests/am1/b030split/margin.py` reads the monitor rows and
  fits the margin's hourly trend by least squares, then projects when the leader's truncation crosses it.
  - To 03:58Z the margin fell 463 an hour over the last 1.5 h (crossing in about 8.8 h), 372 over 2 h (10.9 h) and
    284 over 3 h (14.3 h).
  - The 13-minute orb outage gave it back: no pushes came, site c caught up, and the 04:24Z row reads 4,929. So the
    projection restarts from there.
  - If it crosses, site c's member should restore from the archive as designed, which also exercises `RestoredTo`
    under load. I record its start, its duration, the member's lag after, and `SQLITE_FULL`, and report at once if
    the restore fails or loops. Nothing is changed for it.

### Site c crossed, and the parent stopped committing (2026-11-16, simulated day 25.4 to 25.83)

- **The crossing.** Site c's margin reached zero at about 08:00Z (`b030r2-restore.jsonl`). The leader's
  `truncatedTo` was 199,957 at 08:00:26Z, and site c's applied stopped at 199,808, where it still stands.
- **The install restarted on every archive run.** From 08:00Z, site c's SQLite climbed and fell back five times, for
  example 5.70 → 5.95 GB and then 5.70 GB at 08:04:58Z. Each fall came at a new leader `truncatedTo` (200,121,
  200,519, 200,694, 200,861, 200,984). Each truncation names a new install source, and `load::want` starts a new job
  for a new source (`workers/site/src/load.rs`, `want`), which drops the scratch tables.
  - While the load runs, the leader archives every few minutes, and a 3.25 GB load takes hours, so under load this
    install never finishes [inference from the code and the five restarts].
  - Since the parent stopped committing at 08:14Z, the source has held still at `through` 200,984. That job has run
    since about 08:13Z.
- **The install reloads the metadata tier the parent dropped.** The source is the core snapshot to 200,000 (27.6 MB,
  loaded) and the parent's last metadata snapshot, to 159,387 (3,249,617,556 bytes). That metadata snapshot predates
  `MetaDropped`.
  - `load::source` takes the newest metadata snapshot at or below `through`, split or not. So a parent member that
    installs after the split loads the whole tier back, into scratch tables beside its own 5.7 GB.
  - At 09:15Z the job stood at 1.20 of 3.25 GB, loading about 19 MB a minute. Site c's SQLite was 7.85 GB at
    09:17:33Z and growing 29.8 MB a minute.
  - At that rate it passes `CHILD_GROUP_AT` (8 GiB) at about 09:42Z and 10 GB, `SQLITE_FULL`, at about 10:29Z,
    before the load finishes [projection].
  - This is a gap in the split's design: after `MetaDropped`, the parent's install source should name no metadata
    snapshot, because the child holds the tier.
- **The parent has committed nothing since 08:14:02Z.**
  - Commit and applied stand at 208,159 on sites a and b.
  - Entry 208,160 is a `Config` entry appended at 08:14:02.248Z in term 153, and it never committed. Terms 154 and 155
    appended no-ops on top of it, and the term-155 leader (site a) has appended to 209,558.
  - Site b's log ends at 208,169. Site b has no leader in its view and keeps asking for pre-votes at term 156, which
    site a refuses as leader (`/elections`). Site c is installing.
  - Since 08:14Z every push answers 503. The shard's catch-up, warm and prsync calls fail on
    `repo:1048590#9#f1: 409` (member not joined), and the first of these was at 08:11:33Z. `batchFail` rose from 0 to
    73 by 08:32Z.
  - I wrote `b030-STOP` at 08:33:02Z. The load ended at day 25.827 with 162,457 pushes, and the drain ended at once
    with nothing carried. 29,279 pushes are still owed.
  - **Cause [inference].** The leader's `/place` re-placed site b's member: `replace_on` commits `Degraded { lost: 1 }`
    and then joins new members of site 1 (`#6#f1` to `#9#f1`).
    - The evidence: the placement's join, `/campaign` and `/place` calls on all three sites at 08:13:51 to 08:14:27Z;
      `/place` on site a at 08:14:22Z; members `#6#f1` to `#9#f1` that answer `{"joined": false}`; and the shard
      naming `#9#f1`. `rtt` on site a has a → b at 123 ms, over `NEAR_RTT`'s 100 ms.
    - The `/join`, `/campaign` and `/place` sequence is the edge's `graftyard_lookup::create`. It runs when a lookup
      finds no leader, which matches the leaderless seconds around 08:14Z.
    - Under `Degraded { lost: 1 }`, an entry commits only when sites a and c both hold it (`quorum::stored`). Site c is
      installing, so nothing commits until its install ends, and `SQLITE_FULL` is projected to come first.
    - I have not read 208,160's body. That it is `Degraded { lost: 1 }` is an inference from the quorum math and the
      logs.
    - One fact cuts against this: a prsync call already failed on `#9#f1: 409` at 08:11:33Z, before 208,160 was
      appended. So `#9#f1` may be a refused try from an earlier placement whose registry row the shard reads. The
      stall itself is measured; who appended 208,160, and why, is not.
  - The watcher raised no alert: a leader existed the whole time, and it alerts on a leaderless gap, not on a stalled
    commit index.
- Nothing hosted was changed. The monitor, cross, watch and restore samplers keep reading.

## B0-30 rerun: verdict (operator decision (b), 2026-11-16 about 09:30Z)

**The row fails.** The parent group stopped committing at simulated day 25.83 and cannot reach day 30 without an
operator stepping in. Its member SQLite on site c passed `CHILD_GROUP_AT`, because its install was loading back the
metadata tier the child group had taken.

Options (a) and (c) would also have been an operator stepping in during a gate row, so a pass after either would not
be a pass as written. Nothing hosted was changed after the stall.

| Gate line | Result |
|---|---|
| Member SQLite under `CHILD_GROUP_AT` (8 GiB, 8,589,934,592 B) | **Fails.** Site c's parent member passed it at about 09:43Z and stood at 10,184,200,192 B (9.48 GiB) at 10:44:47Z, still growing. Sites a and b: 5,346,017,280 and 5,277,802,496 B. The child: 3,836,465,152 B on each of three members |
| Zero `SQLITE_FULL` | Held to the end: 0 on all six members, in the monitor's log search and in Workers Logs to 10:46Z. Site c did not reach the store's limit by the 10:45Z cutoff, so its behaviour at full was not observed |
| Archived entries readable for `RestoredTo` | Not judged: `verify.mjs` was not run, because the group cannot commit. Site c's install had not reached the source's 6,381 archive objects (still on the metadata snapshot, entries 0) |
| A child group takes the tier past the line | Held for the group as a whole: split at day 19.5, child loaded on all three, parent dropped. Site c's member then pulled the tier back in (cause 1) |

### Cause 1: a parent install reloads the metadata tier the child holds

- Site c fell behind the leader's archive horizon at about 08:00Z.
- Its `InstallSnapshot` source (`load::source`) names the newest metadata snapshot at or below `through`. After
  `MetaDropped`, that is the parent's pre-split snapshot to 159,387 (3,249,617,556 B). So the install loads the whole
  tier into scratch tables beside the member's own 5.7 GB.
- The install restarted five times between 08:00 and 08:14Z, once at each new leader `truncatedTo` (a new source starts
  a new job).
- After the parent stalled, the source held still and the job ran without a restart from 08:13Z: 1,517 batches and
  2.74 GB of the 3.25 GB snapshot by 10:43Z, at about 15 MB a minute. Site c's SQLite grew 25 to 30 MB a minute.

### Cause 2: a re-placement's `Degraded { lost: 1 }` cannot commit while site c installs

- **Entry 208,160 is `Degraded { lost: 1 }` [measured].**
  - No route returns an entry's body, so I rebuilt its canonical bytes (`crates/raft/src/encode.rs`) for 196
    candidates. The candidates were `Normal` and `Degraded { lost: 0, 1, 2 }`, with f1 at generations 5 to 11 and the
    placement generation at 5 to 11, all at term 153, index 208,160, `at` 1794816842248, `prev` 7de84e60….
  - Exactly one matched the stored digest `7c6d197d…`: `Degraded { lost: 1 }` over the generation-5 placement (a #1,
    b #5, c #1).
- **Why it cannot commit.** `quorum::stored` for `Degraded` needs every voter but the lost one, so sites a and c. Site
  c has been installing since 08:00Z and acks nothing until its install finishes. Every later entry (no-ops of terms
  154 and 155; site a's log reached 212,292 by 10:45Z) waits behind it.
  - Site b's log ends at 208,169. It sees no leader and asks for pre-votes at term 156, which site a refuses as
    leader.
- **Who wrote it [measured, apart from the trigger].** It was the background `/place` that the edge's
  `graftyard_lookup::create` starts:
  - site a: `/join` twice at 08:13:51Z (71 and 34 ms) and `/campaign` at 08:14:01Z;
  - all three members re-measured round trips from 08:13:52 to 08:14:04Z (about 100 `/ping`s from site a). Site c
    stored 290 and 292 ms;
  - `Degraded` was appended at 08:14:02.248Z;
  - a second `create` came at 08:14:24 to 08:14:27Z (`/join`, `/campaign`, `/place`, and the `/place` call on site a
    ran 19.7 s).
  - The home's stored rtt to site b is 123 ms, over `NEAR_RTT`'s 100 ms for enam, so `place_inner` re-placed site b
    through `replace_on`, which commits `Degraded` first.
  - The tries `#6#f1` to `#9#f1` answer `/rtt` with `[]` (joined, then cleared). `#9#f1` is generation 5 + 4.
  - `replace_on`'s `change(Normal)` waits 20 s for the `Degraded` entry, then fails. It logs nothing on that path, so
    the end of the placement is not in Workers Logs.
  - That a lookup found no leader in the leaderless seconds around 08:14Z, and so ran `create`, is [inference]: a
    `create` runs only when its probe `/read` answers no leader.
- **The `#9#f1` 409s.** The shard's catch-up and prsync to site b named `repo:1048590#9#f1` and got 409 (not joined)
  from 08:11:33Z, 2.5 minutes before this placement.
  - That fits registry-restore's finding (`b365051f`, merged to main in `34253d49`, after this run's deploy): an
    earlier placement's losing try left site b's registry row naming `#9#f1`. I found no log of that earlier placement
    in the windows I read, so it stays [inference].
  - Registry-restore fixes the row. It does not stop a re-placement from committing `Degraded` while a member
    installs.

### Site c at the cutoff (10:45Z)

- Applied 199,808, `installing` true, its one install job still running (no restart, no `abandoned`, no `install
  failed` in Workers Logs, 09:30 to 10:46Z).
- It answered its `/raft` calls (100 in 10:40 to 10:45Z, all ok) and was out of the quorum as an acker, as an
  installing member is.
- Nothing committed was lost: sites a and b hold commit 208,159, and site a's log keeps every uncommitted entry.
- `SQLITE_FULL` never came: at 10.18 GB the store still took writes, so the limit is at least 10 GiB on this account.
  The member's behaviour at full is not measured.

### Run totals

- Simulated day 25.827 of 30. 162,457 pushes; 29,279 owed and not pushed (the drain carried none).
- StepDowns counted to 06:10Z: 901 `CommitErrors`, 16 `LoopLag`, 14 `Confirm`, 8 handovers. Not counted after
  06:10Z. Longest leaderless gap 13 s over 52 gaps.
- Tree changes at days 1.82, 1.905 and 2.005; the pause at day 14.916 (16:16:23Z to 16:36:01Z); the stops at day 17.62
  (cost, then site day 9) and day 25.83 (this stall); the drain carried 0.
- Spend on the meter: $139.70 at 10:39Z, of which the split $42.85 and the slim $13.38. That is under the $145 hard
  stop and the $225 line.
- Wrap-up: services stopped at 10:46Z.
  - `shards.py --teardown` tore down the network's three shard containers twice (200 each).
  - Each time, site a's leader's archive `/promote` (about every 2 min, `leader_due`) started them again within a
    minute, and its catch-up then failed on `#9#f1: 409`. The containers keep coming back while that leader runs.
  - The install stays standing as asked, so stopping them for good needs a hosted change (retire the group, or a site
    script whose stalled leader does not promote).

### Teardown: `repo:1048590` retired (operator option (a), 2026-11-16)

- **Evidence saved first.**
  - `tests/am1/b030split/final.mjs` wrote each live member's final `/status`, `/stats` and size to
    `tests/am1/raw/b030r2-final.jsonl` (committed as `b030r2-final.jsonl.xz`). It also saved site c's last install
    report, from `/fold`.
  - It wrote each parent voter's log from 208,150 to `tests/am1/raw/b030r2-log208160.jsonl`. Entry 208,160 is on sites
    a and b with digest `7c6d197d…`, term 153, `committed: false`.
  - `tests/am1/b030split/digest208160.py` re-derives the match: one of 196 candidates, `Degraded { lost: 1 }` over the
    generation-5 placement.
- **Site c's install ended after the cutoff without finishing.** At 11:05:19Z its job was abandoned: "3 batches began
  without committing: the isolate died under them" (`BATCH_TRIES`). It had loaded 3.17 of 3.25 GB in 1,739 batches,
  and the slowest batch took 26.2 s.
  - The member dropped its scratch tables, back to 5.70 GB from 10.18 GB. It stayed at applied 199,808, and
    `SQLITE_FULL` stayed 0.
  - The leader would have started the install again on its next `InstallSnapshot`. So under this source the install
    loops, and each pass reloads the whole tier.
- **Retired.** At 11:14:15 to 11:14:19Z, `tests/am1/b030split/retire.mjs` ran the test route `/retire` (BF-11's
  lost-member stub: storage deleted, object reset) on six members: parent `#1#f0`, `#5#f1` and `#1#f2`, and child
  `#1#f0`, `#5#f1` and `#1#f2`.
  - The log is `tests/am1/raw/b030-retire-1048590.log` (committed, as is `b030r2-log208160.jsonl`, despite `raw/`
    being ignored). All six answer "not joined".
  - Two generation-1 site-1 objects from before site b's replacement were still joined, with no work: `repo:1048590#1#f1`
    (term 6, commit 1) and `repo-meta:1048590#1#f1` (term 2, commit 1). Both were retired at 11:16:50Z and answer "not
    joined".
- **Swept.** `shards.py --teardown` at 11:17:58Z tore down `git#0`, `git#1` and `git#2#net:1048590` (200 each).
  - At 11:41:51Z, 24 minutes later, `shards.py` lists none running.
  - Workers Logs on sites a to c show no `shard/` call and no `1048590` line from 11:18 to 11:41Z.
- **Meter (run-only, DO and Workers plus containers, R2 not included):**

  | Time | DO and Workers | Containers | Sum |
  |---|---|---|---|
  | 11:05:12Z | $124.00 | $20.02 | $144.02 |
  | 11:13:21Z, before `/retire` | $125.60 | $20.07 | $145.67 |
  | 11:43:35Z, final | $126.56 | $20.10 | $146.66 |

  - With the monitor's R2 estimate ($0.86), that is about $147.5 in all.
  - The rise after 11:13Z is the hour's analytics catching up on what the members had already used, plus the teardown.
    No group object or shard ran after 11:18Z.
  - The $145 hard stop was passed by $0.67 before `/retire` ran, while the operator's decision was being asked for.
- **Still standing.** The owner's `Authz` and `Names` still name `m3org/b0-30-r2` (`repo:1048590`). No delete was
  committed.
  - The next lookup of it, such as a git call or an API read, finds no member joined. The edge's `ensure_group` then
    runs `graftyard_lookup::create`, which bootstraps a new, empty group under the same generation-1 names, as
    `tests/m3/wirefix/recreate.mjs` relies on [from the code; not run].
  - Nothing runs against it now.
  - The R2 archives and snapshots stay in `graft-m1-packs-a` and `-b`.

### Design proposals (not built)

1. **After `MetaDropped`, a parent's install source names no metadata snapshot.**
   - `InstallSnapshot.metadata` is already an `Option`. `load::source` sets it to `None` once the fold's split state
     is `dropped`.
   - The installing member keeps the metadata tables at their slim shape, empty, with `RestoredTo` and the split
     state from the core snapshot's fold. Reads fill slimmed text from the child, as they do on every parent member.
   - Between `MetaSeed` and `MetaDropped`, the parent still names its metadata snapshot, since the child may not
     hold the tier yet.
   - **Test:** a member that falls behind after `MetaDropped` installs with no metadata snapshot read, ends under its
     pre-install size, and answers a slimmed comment's body from the child. A plant that keeps `metadata: Some(..)`
     fails on the bytes read.
   - **Design:** DESIGN Shape (`InstallSnapshot`, `MetaDropped`); the split's install case is not written today, so
     this is a gap, not a contradiction.
2. **A re-placement must not leave the group unable to commit.**
   - `replace_on` commits `Degraded { lost: to }`, whose quorum is every other voter. With a voter installing or
     behind, that is a config no quorum can store. Two changes, either sufficient:
     - (a) `replace_on` and `place_inner` refuse to start, and the daily check skips, while any voter other than `to`
       is not matched within `CATCHUP_MAX` of the leader's last index or is past the install source's `through`. A
       re-placement is a goal, not an emergency, so waiting costs nothing.
     - (b) If a `Degraded` entry has not committed within `CALL_DEADLINE`'s multiple that `change` waits (20 s), the
       leader truncates it from its uncommitted tail and goes back to `Normal`. Raft lets a leader drop its own
       uncommitted config only by a new leader overwriting it, so (b) is a `crates/raft` change that needs the long
       sweep and Porcupine. (a) needs neither.
   - **Tie to `NEAR_RTT` during a leaderless gap:** the trigger was the edge's lookup `create` running in leaderless
     seconds on a group that already exists. It re-ran `/place`, which re-placed a member already over `NEAR_RTT`
     (123 ms) on a 25-day-old group under load.
     - `create` should start `/place` only for a group it bootstrapped in this call (`joined * 2 > members.len()`
       from its own `/join`s), not when the joins answered "already joined".
     - A group the daily check already judged keeps its `near_over` rule.
   - **Test:** a hosted-shaped sim with one voter installing, then `/place`. With (a), no config entry; on the
     current code, a stall.
3. **Registry-restore and `#9#f1`.**
   - Registry-restore (`b365051f`) puts the kept member's row back after a re-placement, so the shard and edge would
     no longer call `#9#f1`. That covers the 409s from 08:11:33Z, if they came from an earlier placement's losing
     try, as its LM-1 round 29 case did.
   - It does not cover cause 2: the row is written only when `replace_on` ends, and here `replace_on` never got past
     its `Degraded` commit.
   - With proposal 2(a) the placement would not have started.

### A full B0-30 rerun

- At this run's rates: about $140 to day 25.8 including the split's $42.85, plus about 4.2 simulated days at $2.29 an
  hour (about 13 h) and an 11 to 12 h drain. project.py's last full projection was $181.7.
- So about **$180 to $185**, plus one site day carrying proposals 1 and 2 (and main's registry-restore, edge
  read-gate retry and disk-validate).
- About 40 h of load and 12 h of drain.
- **Approved (user, relayed by the operator, about 13:40Z 2026-11-16), at about $185.** Hard stop $190 on graft-m1's
  meter, counted from the rerun's first site day; the 24 h drain limit is unchanged. It runs only after:
  1. `b030-fixes` is merged to main: (i-1) a slim snapshot when the slim ends, (i-2) an install extends instead of
     restarting, (i-3) an installing parent member drops its own metadata tables, (ii)(a) no re-placement while a
     voter is installing or behind, and `create` starts `/place` only for a group it bootstrapped;
  2. one site day from main with those fixes and whatever else main holds then (m3f's receipt rule rides along if it
     is merged; the rerun does not wait for it).
  - The row is unchanged: 30 days of journey 9 with pushes on, the 4 GB tier, the split at `CHILD_GROUP_AT`. The
    watcher stays, with one alert added: commit index unchanged for 60 s while a leader exists. Reports at the
    split, on any stop, and every 4 h.

### b030-fixes (built 2026-11-16, branch `b030-fixes` off main 2465047d)

Design first: DESIGN Shape, `CHILD_GROUP_AT`, "Installs." states the three install rules and the drop condition.
DECISIONS has "An installing parent member drops its own metadata tier…" and "A re-placement waits for every voter…".
No `crates/raft` change; (ii)(b) stays unbuilt.

- **(i-1) A slim snapshot when the slim ends.** `split::slim_snapshot_due`: the metadata tier is due a snapshot once
  the slim cursor is done and the newest metadata snapshot's `to` is below the entry that wrote the done cursor.
  `snap::begin` takes it. So an install's newest metadata snapshot at or below `through` is the slimmed tier.
- **(i-2) An install extends instead of restarting.** `load::next_job(running, new)`: a source with the same core and
  metadata snapshot keys, a `through` not earlier, and archives that continue the running job's extends it to the new
  `through` and answers the newer round. Any other source restarts it and logs "install restarted".
- **(i-3) An installing parent member drops its own metadata tables.** Condition (`drop_due`): the member is past
  `MetaDropped`, its install job is stored and started, and its `applied` is below the source's `through`. The job
  records the tables and a `meta_dropped` row in its own transaction, unloads the tier, then drops one table a batch
  (`tier::drop_live`). While the mark stands:
  - `family::meta_loaded` is false on every restore, so `follower_query` serves no metadata or narrow `api` read
    (the edge moves on);
  - `snap::load_meta` replays nothing, and `snap::tiers_json` answers `at: null`, so the fold audit lists it as
    loading and flags nothing;
  - `Family::may_lead` is false: `/campaign` and `/suspect` answer `mayNotLead`, `keep_led` skips it, and a
    `TimeoutNow` is answered `TransferRefused`.
  - `Write::Install` clears the mark with the table swap. An abandoned job leaves it, so the member stays unloaded
    until the next install finishes. `NotNeeded` swaps the scratch tier in, clears the mark, and leaves the tier
    unloaded for `load_meta` to replay above `through`.
- **(ii)(a) No re-placement while a voter is behind or installing.** `replace_on` reads `/status` `applied` from each
  voter `Degraded { lost: to }` would need (not itself, not `to`). `degraded_needs` builds `Ready` only if each is
  within `PLACE_LAG_MAX` (64) of the leader's commit; an unanswered voter counts as behind. Otherwise it answers
  `{"error": "behind"}` and changes nothing.
- **create→/place.** A member's `/join` sets `x-graft-joined-fresh` only when that join built its core.
  `graftyard_lookup::create` places only when a majority of its bootstrap joins were fresh
  (`bootstrapped_here`). B0-30's 08:14Z case (every join "already joined") now places nothing.

**Tests, and whether each fails on main.** Main was checked in a worktree at 23ce5dc4 with the tests ported and
stand-ins giving main's behaviour (main has none of these functions).

| Test | File | On main |
|---|---|---|
| `the_slimmed_tier_is_snapshotted_once_its_pass_ends` | `crates/git/tests/split.rs` | fails |
| `a_snapshot_at_the_pass_end_satisfies_it` | `crates/git/tests/split.rs` | (companion) |
| `a_later_source_with_the_same_snapshots_extends_the_running_install` | `workers/site/src/load.rs` | fails |
| `a_source_with_other_snapshots_or_a_gap_restarts_the_install` | `workers/site/src/load.rs` | passes (main always restarts) |
| `only_an_install_on_a_split_parent_past_the_horizon_drops_its_tier` | `workers/site/src/load.rs` | fails |
| `a_member_with_its_tables_dropped_is_never_loaded` | `workers/site/src/load.rs` | fails |
| `an_unloaded_tier_answers_no_digest_for_the_audit` | `workers/site/src/load.rs` | fails (main's real `tiers_json`) |
| `a_member_that_may_not_lead_refuses_a_transfer` | `workers/site/src/member.rs` | fails |
| `a_degraded_config_waits_for_every_voter_it_needs` | `workers/site/src/member.rs` | fails |
| `only_a_group_this_create_bootstrapped_is_placed` | `crates/lookup/src/lib.rs` | fails |

**Plants.** `tests/b030fixes/plant.py`: 20 mutants, 20 caught. (i-1) 3, (i-2) 5, (i-3) 7, (ii)(a) 3, create 2. One
(an extended job keeping its old `through`) survived the first test and is caught after the test gained the
"only the metadata snapshot differs" and later-`through` cases. `plant.py --residue` runs in the full checks.

**Disk peak [measured, local workerd].** `tests/b030fixes/dropbed.js`: a 256 MiB log plus a 512 MiB metadata table
(918,556,672 B), then a 512 MiB scratch tier. Kept: peak 1,531,289,600 B. Dropped first: 306,192,384 B after the
drop, peak 918,921,216 B. So the peak is own − metadata + scratch. Site c's case [inference]: 5.70 − 3.84 + under
1 ≈ under 2.9 GB with (i-1)'s slimmed snapshot, about 5.7 GB without it, against 10.18 GB measured on the rerun.

## B0-30 third run (b030r3, from 2026-11-16 17:25Z)

Approved by the user (about $185, relayed by the operator 13:40Z). b030-fixes merged to main as `0e3d13cc`
(operator, full checks on the merged tree: 1,054 passed, 0 failed).

- **Site day r3, announced 17:22Z, start 17:25Z** (`tests/am1/b030split/siteday-r3.sh`, log
  `tests/am1/raw/b030r3-siteday.log`):
  - Sites a to d from main `0e3d13cc`: b030-fixes, with main's receipt rule (`8473e0ea`), push-reads and the rest.
  - New git shard image from the same commit's `containers/git`, tag `b030r3`, `sha256:c38059db…5668`
    (`tests/am1/raw/b030split-image-r3.log`). Main's container changes since site day 9's image: repack-keep-pack,
    composed-hits and composed-miss, promote-unlock.
  - `SHARD_INSTANCE` standard-4. Other variables as site day 9 (`GRAFTYARD_AUDIT`, request retention 9,600 s).
    `--roll-shards` (none are running).
  - Then the edge `graft-m1-edge` from the same commit, with `tests/am1/hosted/deploy-edge.sh`'s flags. The edge
    runs `graftyard_lookup::create`, where the create→/place rule lives, and has run `19b495a1` since 2026-11-13.
  - Every member resets once per site. No new class, binding, migration, format or event tag.
- **Row as written.** A fresh repository `m3org/b0-30-r3` seeded with the 4 GB metadata tier, then 30 simulated days
  of journey 9 at 18x with pushes on (1.6 a second, 2.48 GB a simulated day), no keeper and no keepHome, the split at
  `CHILD_GROUP_AT`, then the drain. Drivers `tests/am1/b030split/rerun.sh` with `B030_RUN=b030r3`.
- **Stop rule.** $190 on graft-m1's run-only meter, counted from the site day's start (`tests/am1/raw/b030r3.start`).
  Containers count install-wide, pool shards included, not only the run's network (`B030_ALL_CONTAINERS`). Drain
  limit 24 h. The watcher adds a stall alert: the leader's commit index unchanged for 60 s (`STALL_S`), alert only.
- **Reapers and sweeps.** graft-m1 runs no reaper (`tests/fanout/reaper.sh` and the LM-1 reapers act only on
  `graft-m2b`). The end-of-run checks cover every graft-m1 shard by name, not by range: `shards.py` lists every
  running `graft-m1-site-*` instance, network or pool, and `tests/fanout/teardown.py --sweep` (`INSTALL=graft-m1`)
  takes every network and pool. At 17:05Z no graft-m1 shard was running. The second run's generic state files are
  in `tests/am1/raw/b030r2-generic/`.
- **After the run:** `/retire` on the six members, a sweep, and the meter checked flat for 30 minutes.

### Third run: site day, seed and the first hour

- **Site day r3 done 17:17:28Z to 17:33:22Z, exit 0** (`tests/am1/raw/b030r3-siteday.log`): sites a to d in two
  phases (sites exit 0 at 17:30:55Z), then the edge, version `83ec5e95`. No shard was running, so the roll had none
  to kill.
- **Seed.** `m3org/b0-30-r3` is `repo:1048591` (main `cd7fe7a`). 1,340,000 comment rows of 1,800 B on each member,
  17:36:41Z to 18:12:34Z, SQLite 4,017,299,456 / 4,020,723,712 / 4,017,356,800 B, metadata digests equal
  (`59c7b45d…`). Two `/bench_seed` calls failed on the client side (a reply without the digest) and were retried.
  Spend at the end of the seed: $4.28.
- **Load from 18:12:55Z.** First hour (to 19:19Z, simulated day 0.83): 95.3 to 95.9 pushes a minute, `batchFail` 0,
  push p50 1.8 s at the start and 3.9 s by 19:19Z (max 9.4 s). Archiving began at about 19:00Z with the log at
  1.93 GB (`LOG_MAX_BYTES` 2 GiB): at 19:10Z archivedTo 1,160, promotedTo 6,166, log 2.004 GB; at 19:13Z log
  1.899 GB and 775 archive objects. Member SQLite 6.08 GB. `SQLITE_FULL` 0, bodies baseline 100.
- **Site day checks.** `/status` shows `procs` on sites 0 and 1's shards (site 2's had not started). In 19:20 to
  19:23Z, site a's push validates: 129, p50 290 ms; `/merge` 58, p50 789 ms. One leaderless gap of 11 s
  (19:15:47Z to 19:15:58Z, leader to site 1), under GAP_MAX_S. No stall alert.
- **hourly.py fix.** Main's receipt rule puts `record=` before `tipsn` in a push validate's URL, so the old
  `validate?repo=N&tipsn` filter matched none (18:00Z's row reads 0). It now matches on `tipsn=` anywhere.

### Third run stopped by the operator (2026-11-16 20:13Z, account over budget)

The operator stopped all hosted work: the account's bill for Oct 27 to Nov 16 is $2,328 against an $800 budget
($1,349 containers, $804 DO rows written). The third run ends at simulated day 1.2 with no verdict.

- **Stopped at 20:13:23Z.** `b030-STOP` was written and the services stopped: load, drain, cross, watch, monitor,
  seed and siteday. The watch schedule is paused. At the stop the run's meter read $5.55 (DO and Workers $4.4, plus
  containers). The group was healthy: SQLite 6.30 / 6.30 / 6.34 GB, log 1.97 to 2.04 GB, archivedTo 7,880,
  `SQLITE_FULL` 0 (`tests/am1/raw/b030r3-final.jsonl`).
- **Containers.** `teardown.py --sweep` (`INSTALL=graft-m1`, pools included) tore down `git#0` and `git#1` of
  net:1048591, both standard-4 (200 each). At 20:14:12Z the Containers API listed 0 running across all 8 graft-m1
  apps (site-a to d, lin-a to c, wp6-bed), and 0 again at 20:15:40Z, 20:22:13Z and 20:26:09Z. graft-m1 has no
  Sandbox or runner app.
- **Retired with `/retire`** (`tests/am1/b030split/retire-r3.mjs`, log `tests/am1/raw/b030-retire-r3.log`), every
  member answering "not joined" after:
  - 20:16:48Z: `repo:1048591` (`#1#f0`, `#1#f1`, `#1#f2`). It had no child group yet.
  - 20:25:53Z, B0-30's earlier test groups, still joined and idle: `repo:1048585` (3), and `repo:` and
    `repo-meta:` for 1048587 (smoke 2), 1048588 (smoke 3's first try) and 1048589 (smoke 3), 18 more. That is 21
    in all. 1048586 and 1048590 were already retired.
- **Still joined, not retired.** Every other group on graft-m1 belongs to earlier AM1 work, not B0-30: site a homes
  148 repository chains. The fold audit (`*/15`, 4 chains a run, `GRAFTYARD_AUDIT=1` since site day 9) audits them.
  **Corrected 2026-11-16 21:40Z:** I reported that it failed on repo:489004 to 489007 every run and started over on
  them. That was wrong; I had read one 20-minute window. Workers Logs for site a, 00:00 to 21:30Z, 86 runs: each run
  took the next four chains after the stored cursor, all 148 in turn, a full pass about every 9 h 15 min (repo:196619
  at 00:03, 09:18 and 18:37Z), and a chain that failed did not hold the cursor. Of 348 chain reports, 25 compared
  digests, 179 failed on a call (160 a 404 on a retired or never-placed member, mostly the f2 member of a removed
  test group), 141 carried a refold that ended `SQLITE_LOCKED`, and 3 had no report yet. Sites b and c home none.
  So it is not a loop on the same chains; its cost is about 10 rows a run on site a.
- **The SQLITE_LOCKED refolds: cause and fix (branch `audit-locked`, 2026-11-16).** A SQLite cursor stopped
  before its last row keeps its statement, and a lock on what it read, until the isolate collects it. Until then
  every `DROP TABLE` in the object fails with SQLITE_LOCKED. That includes the fold audit's `reset_scratch` at its
  first batch, which is where the refolds failed. The repeat offender is the archive alarm. Every minute, a leader's
  `SiteHost::view` walks the log until it passes promote-ahead's bound, and `SiteHost::rows` walks it until one row
  past the due range. Both are lazy cursors that stop early.

  Local workerd (`tests/auditlock/bed/run.sh`, 200 log rows on a fresh object each time, measured):

  | Read | Then | Main's reads | This branch |
  |---|---|---|---|
  | `view` + `rows` | audit `DROP TABLE`, next request | `SQLITE_LOCKED` | ok |
  | `view` + `rows` | the same, one request later | `SQLITE_LOCKED` | ok |
  | `each_row` stopped after 1 row | `DROP TABLE`, same request | `SQLITE_LOCKED` | ok |

  The fix is `workers/site/src/logread.rs`:
  - `log_after` reads the log in pages, each query read to its end. Pages start at 8 rows and grow while they hold
    under 2 MB, up to 1,024 rows. The archive walks, `stored_range` (catch-up) and `entries_json` use it.
  - `each_row` steps through the rest of a statement it stopped early, without keeping the rows.

  Two plants fail the bed:
  - `each_row` not draining: `each_row` drop `SQLITE_LOCKED`.
  - `log_after` taking a page without reading its query to the end: audit drop `SQLITE_LOCKED`.

  Ships on the B0-30 short run's site day.
- **What was writing [measured, `durableObjectsPeriodicGroups` by namespace, every graft-m1 namespace].**

  | Window | Rows written an hour | DO-hours an hour | What |
  |---|---|---|---|
  | 12:00 to 16:00Z | about 4,200 | 1.5 | idle: 669 objects, at most 152 rows each in 4 h |
  | 17:00Z | 11.1 M | 2.5 | the seed's 1.34 M rows of 1,800 B on three members |
  | 19:00 to 20:00Z | 1.55 M | 21.3 | the load: three members of repo:1048591 |
  | 20:15 to 20:30Z | 37,600 (9,395 a quarter) | 2.0 | after the stop and the first retire |

  - graft-m1 wrote 92.5 M rows ($92.54) and 318.4 DO-hours from 2026-11-15 20:00Z to 2026-11-16 20:00Z. Nearly all is
    the B0-30 runs: the second run's load and split (2.4 to 2.7 M an hour, 22 DO-hours an hour, through 07:00Z), site
    c's install (10.1 M at 08:00Z, 5.0 M at 10:00Z, 3.7 M at 11:00Z), the third run's seed and load. When no run was
    going (12:00 to 16:00Z), it wrote about 4,200 rows and 1.5 DO-hours an hour.
- **The run meters undercount.** graft-m1 wrote 419.7 M DO rows from Nov 1 to 16 ($419.71), and its containers
  billed $852.62 from Oct 27 (the leak, Nov 6 to 13, $802). B0-30's meter counted only the run's own objects
  (`objects.py` from Workers Logs) and its own network's containers. The second run's meter read $118.95 of DO and
  Workers at 10:39Z, with 117.3 M rows counted, against 154.2 M rows for all graft-m1 namespaces in the same
  window (2026-11-14 09:46Z to 2026-11-16 11:44Z). So objects the logs did not name, such as smoke groups and earlier generations, were off the
  meter.

- **Idle reading, 30 minutes after teardown (rule 3, `acctcost.py`).** 20:30 to 21:00Z: the whole account $0.11,
  graft-m1's lines $0.01 (site a $0.01, sites b to d under $0.01), so about $0.02 an hour for graft-m1. Its
  namespaces wrote 1,886 rows in 20:30Z and 1,090 in 20:45Z, about 6,000 an hour, with 0.83 DO-hours in the 30
  minutes. 0 containers running at 21:11Z. The third run's close-out is done.

## B0-30 re-judged at 90% (operator decision, 2026-11-16, `FRAME.md`, "90% gates and a daily spend limit")

No spend. Read from the results above and `tests/am1/results/b0-30.md`. The row's targets are 30 simulated days of
journey 9 with pushes on (1.6 a second), member SQLite under `CHILD_GROUP_AT` (8,589,934,592 B), zero `SQLITE_FULL`,
and archived entries readable for `RestoredTo`; its fail line is any `SQLITE_FULL`, or SQLite over `CHILD_GROUP_AT`
without a child group taking the tier. At 90%: 27 simulated days, a size bound of 1.1 × `CHILD_GROUP_AT`
(9,448,928,051 B), and the push rate at 1.44 a second.

**Still short, on every run. No row status changes.**

| Run | Days (target 27) | Max member SQLite (bound 9.45 GB at 90%) | `SQLITE_FULL` | `RestoredTo` | Short by |
|---|---|---|---|---|---|
| First (`b0-30.md`, 2026-11-11) | 27.52 | 8,591,708,160 B, 1.0002 × the line | 0 | 81 of 81 ranges, 10 of 10 lookups | Pushes off from day 1.34: 7,853 pushes, 3.7% of journey 9's 1.6 a second. And no child group took the tier, so it hit the fail line as written. A size within 10% does not lift a fail line: the operator's rule relaxes targets, not the "without a child group" clause |
| Second (`b030r2`, 2026-11-14 to 16) | 25.83 (86%) | 10,184,200,192 B on site c, 1.186 × the line | 0 | Not judged: the parent could not commit, so `verify.mjs` did not run | Day 25.83 of 27, about 1.2 simulated days short. Site c 735 MB over the 90% bound. And the parent stopped committing at entry 208,160 (`Degraded { lost: 1 }` waiting on an installing voter), so the group is not live: that is a quorum stall, not a target missed by a margin |
| Third (`b030r3`, 2026-11-16) | 1.2 | 6.34 GB | 0 | — | Stopped by the operator at day 1.2, no verdict |

- The second run's push rate also fell short of 90% (86.4 a minute) for long stretches: 96 a minute at the start,
  66 by day 14.9 (G14), 60.5 before the stop at day 17.62, 79 to 80 after site day 9. It pushed 162,457 of the
  191,737 due by day 25.83, 85%.
- Not an invariant failure on either run: no acknowledged push was lost, and the quorum rule held (entry 208,160 did
  not commit because the rule held).
- **What it needs.** A run to day 27 with the parent live and every member under 9.45 GB, plus a `RestoredTo`
  verify. b030-fixes (on main since `0e3d13cc`) targets both of the second run's causes; the expected site c peak
  with them is under 2.9 GB [inference]. So B0-30 still needs a rerun under the operator's slot rule. At 90% the run
  could end at simulated day 27 (36 h of load at 18x, against 40), which saves about 10% of its load cost.
- **Cost of the second run on the account meter** (`tests/fanout/acctcost.py`, every `graft-m1-*` line, 2026-11-14
  09:46Z to 2026-11-16 11:44Z): $190.43 (members $167.46, shards $22.78, edges $0.19), against the run meter's $139.70
  for nearly the same window. The third run, 17:17 to 20:15Z: $16.83 for graft-m1 (account $20.44).

## Proposal: a shorter B0-30 run from the split onward (2026-11-16, not run)

The second run's two failures both come after the split: site c's post-split install (cause 1) and the
`Degraded` re-placement while it installed (cause 2). The 19.5 simulated days before the split show only the parent
growing to `SPLIT_AT` (0.0585 GB a day, measured on both runs), and they cost $110.18 of the second run's $190.43
(`acctcost.py`, graft-m1 lines, 2026-11-14 09:46Z to 2026-11-15 23:45Z, the seed included).

**Run (option A, recommended).**
1. Site day from main (b030-fixes in), as the third run's.
2. Seed a fresh group's metadata tier to about 5.5 GB (`seed.mjs 5500000000`), so that with the log at its 2 GiB cap
   the parent passes the real `SPLIT_AT` (7 GiB) about 0.8 simulated days into the load. The third run's 4.02 GB
   seed reached 6.08 GB at day 0.75. No product constant changes. The tier is bigger than the row's 4 GB, which makes
   both the child and an unfixed install's scratch bigger, so it is no easier on the gate line.
3. Journey 9 with pushes on, as written, until 8 simulated days after `MetaSplit`: the second run's post-split span
   to day 27 was 7.5.
4. Force the second run's lag instead of waiting for it: after `MetaDropped`, while the slim runs, `/sever` site c's
   parent member (LF-13's test route) until the leader's `truncatedTo` passes its applied (about 7,000 entries, 75
   minutes at 95 a minute), then lift it. That puts site c through an install from a pre-slim snapshot, the second
   run's case, under load and through archive runs, which exercises (i-2) and (i-3); a full run lags only by chance.
5. While site c installs, call `/place` on the parent's leader as the edge's `create` would: (ii)(a) must answer
   "behind" and append no config entry.
6. End: `fill.mjs` on 10 or more slimmed rows, `verify.mjs` for `RestoredTo`, then the close-out of rule 3.

**What it judges.** The gate line as written (every member under `CHILD_GROUP_AT`, zero `SQLITE_FULL`, `RestoredTo`
readable, a child group holding the tier), plus: site c's install peak against own − metadata + scratch, no
`install restarted` across archive runs, no metadata read or lead from site c while marked, the 60 s stall alert
silent, and no `Config` entry from step 5.

**Cost on `acctcost.py`, graft-m1 lines, scaled from the second and third runs [estimate].**

| Phase | Basis | Estimate |
|---|---|---|
| Site day | third run's 17:17 to 17:36Z: $0.00 | $0.5 |
| Seed 5.5 GB | third run's 4.02 GB seed, 17:36 to 18:13Z: $12.75, × 1.37 | $17.5 |
| Load to the split, about 1.2 h | third run's 18:13 to 20:13Z: $2.02 an hour | $2.5 |
| Child restore | second run's 23:45Z to 00:45Z: $33.29, less $3 of load, × 1.37 | $41 |
| Load after the split with the slim, about 10.7 h | second run's 00:45 to 08:00Z: $3.33 an hour | $36 |
| Site c's install of the pre-slim tier | second run's 08:00 to 11:44Z: $22.81 with the load stopped, × 1.2 | $27 |
| Verify, teardown, 30 min idle | | $1 |
| **Total** | about 14 h of wall time | **about $125** |

**Option B, about $100.** Keep the row's 4 GB seed and set the split threshold with the test-only `/bench_seed
{"splitAt"}` to the parent's size once the log is full (about 6.1 GB); everything else as A. It still tells the fix
from its absence: an unfixed install would peak about 6.1 + 4.5 = 10.6 GB, over the line, and a fixed one about 3.3
GB. But it moves a product threshold, and `splitAt` lives in each isolate, so the driver must set it again until
`MetaSplit` commits.

**Why not the full 27 days.** Nothing in the row depends on the pre-split days beyond reaching `SPLIT_AT`. `LOG_KEEP`
(30 days of real time) never applies at 18x. Archiving held the log at its cap for days 7 to 22 on the first run and to
day 25.8 on the second. The post-split span is kept. A full run would also exercise the install only if a member
happened to lag. Whether the shorter procedure counts as the row as written is the operator's decision.

## B0-30 short run (b030r4, from 2026-11-17 00:02Z)

- **Site day:** from main `d870e2cc` (with `audit-locked`, `near-check`, `lookup-share`), 00:02:21Z to 00:16:46Z,
  sites and edge exit 0. acctcost for the day: $0.01 before, $0.06 after.
- **Tools edge redeployed (operator, 2026-11-17 00:4xZ).**
  - The seed failed at once in `mkrepo.mjs`: every call to `graft-m1-wp7-edge` answered a 404 page. The
    account's audit log shows `script_delete` of `graft-m1-wp7-edge` and `graft-m1-wp7-edge-b` at
    2026-11-16T20:28:53Z, in the cost teardown.
  - The run's drivers need it: `mkrepo` (the names-shard commit), `force` (`/sever`, `/place`) and retire.
  - Redeployed `graft-m1-wp7-edge` only, with `SITES=tests/m3/finish/sites4.json EDGES=graft-m1-wp7-edge node
    tests/m1/wp7/deploy-edge.mjs` and one `wrangler secret put TOKEN` from the token file, not printed.
    `graft-m1-wp7-edge-b` stays deleted; no driver calls it.
  - Upload version `bbe289d8-3962-4321-99d5-a34438b9589a`; deployed version after the secret
    `c2f03106-b84e-4517-835f-edc5f680d0ea` (2026-11-17T00:36:21Z).
  - Checked through it before the seed restarted: `/status` on all three members of `authz:7100001`, each joined,
    commit 312, 311 and 311.
  - Kept after the run: retire needs it.
- **Order of events** (`raw/b030r4-force.jsonl`, `-watch.log`, `-seed.log`, `-load.log`):
  1. 00:38 to 00:58:54Z: seed of repo:1048592 (m3org/b0-30-r4). 1,840,000 rows of 1,800 B; each member at
     5,516,148,736 B with equal digests.
  2. 00:59:34Z: load starts, journey 9 at 18x, pushes on.
  3. 01:43:47Z, day 0.551: `MetaSplit` committed, child `repo-meta:1048592` seeded from the metadata snapshot at
     2,638. The leader (site 0) was at 7.53 GB. The run ends at day 8.551 (`b030-days`).
  4. 01:45 to about 02:10Z: the child's three members restore the snapshot, about 9.2 M rows written, $9.6 for
     01:45 to 02:00Z on acctcost.
  5. 01:58:59Z: one leaderless gap of 5 s, under the 30 s stop.
  6. 02:05:21Z: `MetaDropped` on the leader, with the slim pass started (table 0). Site c (site 2) was at applied
     6,889, 467 behind the leader's 7,356, with its own lag growing, but the leader's truncatedTo (2,411) was far
     below it. So c did not pass the horizon on its own before `MetaDropped`. The operator's unforced-lag case
     (2026-11-17 02:10Z: let the install run, skip the sever, and run `/place` during it) did not arise, and force's
     sever of site c began at 02:05:21Z as designed.

## Cost

| Window | DO and Workers | Containers |
|---|---|---|
| Nov 12 00:00Z to Nov 13 16:45Z (the leak) | $5.74 | $171.42 |
| Nov 13 19:45Z to 23:03Z (smoke 1, this thread) | $58.61 | $3.31 |
| Nov 13 23:03Z to Nov 14 00:20Z (idle split group) | $0.04 | $0.51 |
