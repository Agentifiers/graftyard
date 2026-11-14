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
`split::tests::a_split_needs_its_leader_only_while_a_step_is_due`. Plants 15 to 24 in `tests/am1/b030split/plant.py`
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

## Smoke 2: split and slim with no keeper

Not run yet.

## B0-30 rerun

Not run yet.

## Cost

| Window | DO and Workers | Containers |
|---|---|---|
| Nov 12 00:00Z to Nov 13 16:45Z (the leak) | $5.74 | $171.42 |
| Nov 13 19:45Z to 23:03Z (smoke 1, this thread) | $58.61 | $3.31 |
| Nov 13 23:03Z to Nov 14 00:20Z (idle split group) | $0.04 | $0.51 |
