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

## Smoke 3: split and slim at `SLIM_BYTES`, no keeper

Not run yet.

## B0-30 rerun

Not run yet.

## Cost

| Window | DO and Workers | Containers |
|---|---|---|
| Nov 12 00:00Z to Nov 13 16:45Z (the leak) | $5.74 | $171.42 |
| Nov 13 19:45Z to 23:03Z (smoke 1, this thread) | $58.61 | $3.31 |
| Nov 13 23:03Z to Nov 14 00:20Z (idle split group) | $0.04 | $0.51 |
