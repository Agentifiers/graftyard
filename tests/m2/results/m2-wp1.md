# M2-WP1: folds in member SQLite, hosted (BF-20)

Run on 2026-11-03 against the test install `graft-m1-site-a/b/c` (enam, weur, apac; site ids 0, 1, 2). The build was `test-faults`, from branch `m2-wp1-hosted`. The gate row is in `build/BUILD-PLAN.md`, M2-WP1:

> BF-20 | Snapshot of a 4 GB [guess] metadata tier under 300 commands/s | Apply p99 within 10% [guess]. Snapshot digest equals a refold at `to`. No request-limit errors | Apply p99 more than 10% slower, a digest mismatch, or any request-limit error

| Clause | Result |
| --- | --- |
| A 4 GB metadata tier in one member | Seeded to the 4,294,967,295-byte reporting ceiling (about 4.3 GB) on all three members of `repo:196627` with equal `fold_digest`. Snapshotted at 1.33 M rows, 3.99 GB of member SQLite, on `repo:196628`. See "Data size" |
| 300 commands/s | Offered 300/s. With no snapshot running, the group carried all of it in 1 of 3 runs. The other 2 lost their leader on `CommitErrors` (far AppendEntries past `CALL_DEADLINE`). With the snapshot running, it carried all of it in 4 of 4 runs. See "Load" |
| Apply p99 within 10% of the no-snapshot baseline | **Pass at 150/s, unclear at 300/s.** The apply step stayed under 1 ms on every member in both phases. Leader entry age p99 was +1.7% at 150/s. At 300/s it was +30%, on one sample per phase, with p50 30% lower. See "Load" |
| Snapshot digest equals a refold at `to` | **Pass.** `snapcheck` refolded all 1,330,000 rows of the R2 object and got the trailer's digest, which is the key's digest, which is the `fold_digest` `apply` kept at `to` = 18 |
| No request-limit errors | **Fail.** On `repo:196628`, the apac member exceeded its memory limit 4 times during the snapshot (`exceededMemory`, on `/read` and `/snapshot_commit`), after its restore threw a `Critical error`. `repo:196627` did the same twice earlier. See "The defect" |
| **Verdict** | **Fail.** The snapshot itself works: digest exact, apply not slowed at 150/s. But members of the group hit request limits after a restore, and the group stopped serving, at about 63,000 to 80,000 commands. The cause is not established |

## What was built

The hosted half of M2-WP1. Commits on `m2-wp1-hosted`, in order:

- `7bc7874` Raft. `Fold::apply_entry(&LogEntry)` hands a fold the whole committed entry. Its default applies each event, then the `IdBlock`. `Fold::scratch_at(at, tiers)` gives `decide` and `verify` the entry's time and tier tag. `Fold::tiers(event)` tags recorded entries. `GroupFold::apply` calls `apply_entry`.
- `97eeac4` Fold. `TierStore`, `Store::clear`, `Meta.at`, and `snap_sum`. A resumable `SnapshotCursor` with `want()`. `FrameCheck` and `decode_some` for streamed snapshots, `encode_frame`, the per-tier digest ring (`digest_at`, `DIGESTS_KEEP` = 20,000), and `seed()` for test installs.
- `9ae8381` Git and site. `RepoFold` on two tiers in member SQLite, `TierSql`, snapshots, and the fold audit.
- `3490101` Site. Keeps the test-only strings out of the product build.
- `ba2c186` Fold. `Overlay::scan` copies only the overlay rows a scan can return, and the name-conflict check reads 8 refs at a time. One entry of 2,000 single-ref pushes went from 0.38 s to 8 ms in native `decide`.
- `2ba89d5` Site. Hosted fixes and the BF-20 instruments.

### The Raft hook

`graftyard_raft::Fold` gained three defaulted methods, and no existing fold changed:

- `apply_entry(&mut self, e: &LogEntry<Event>)`: the whole committed entry, with its index, `at`, `tiers`, and requests. The default is the old loop of per-event `apply`, then `apply_id_block`, so `NamesFold`, `TokensFold`, `AuthzFold`, `IdsFold`, `Reg`, and `ReleaseState` behave exactly as before. `GroupFold::apply` calls it.
- `scratch_at(&self, at, tiers) -> Scratch`: the scratch view `decide` and `verify` run over. The leader passes its `now` and every tier. A follower's `verify` passes the entry's own `at` and `tiers`. The default is `scratch()`.
- `tiers(event) -> TierSet`: which tiers an event writes, so `Member::record` can tag a recorded entry the way `decide` tags a decided one.

This is the smallest hook that lets a tiered fold see what `apply` needs: the entry's index (each tier refuses an entry that does not follow its `applied`), its time (pruning by entry time), and its tier tag (a metadata tier that is loading skips it). `decide` stays pure, because the scratch it reads is a function of the fold, the entry time the leader stamps, and the earlier commands of the entry. The `Command` trait did not change. The long sweep and Porcupine stayed green; see "Checks".

### Where refs live

`RepoFold` holds only the two `TierStore`s. Refs, `promoted`, and `archived` are rows of the core tier. `Push::decide` and `verify` run on `RepoScratch`, an `Overlay` over the core tier that the same `RepoTiers::write` writes as `apply` does. So `decide` reads the same `ref` table that `apply` writes, and there is one source of truth for refs.

The rejected alternative was to keep `RepoFold`'s in-memory ref map for `decide` beside the core tier's `ref` table. Then a restore, a snapshot install, or a lazy load would have to keep two copies equal, and `fold_digest` would cover only one of them.

The change is in `crates/git/src/repo.rs` and `crates/git/src/tiers.rs`. In `tiers.rs`, which the other agent owns, the CORE schema gains three tables: `promoted`, `archived`, and `snapshot(tier, to)`. It also gains `snapshots()`, and pruning of `snapshot` rows by `snapshots_to_keep` when an `Archived` for a snapshot applies. The issue tables are untouched.

### The SQLite store

`workers/site/src/tier.rs`, `TierSql`, one handle per tier over the member's own SQLite:

- **Tables.** The DDL is generated from the tier's `Schema`. Each table is `c_<name>` or `m_<name>`, with its key columns `k0..kn` under a composite primary key, `written`, and `v`, the row's columns in postcard. A keyless table holds one row. Beside each table there is its `snap_undo` table (`<t>_<name>__undo`), the tier's meta row (`<t>__fold`), and its digest ring (`<t>__digest`). A schema change is a new table set, never an `ALTER`.
- **Search.** The metadata tier has two FTS5 tables, `m_issue_fts(title, body)` and `m_comment_fts(body)`. They are contentless with `contentless_delete=1` and keyed by the issue or comment number. They are derived: not in `fold_digest`, and not in a snapshot. Locally, FTS5 added about 30% to the tier's bytes.
- **Writes.** `apply` writes into a buffer. `seal` turns the buffer, and the FTS5 rows it implies, into statements inside the input's one transaction, beside the `raft_state` row that says the entry applied. Reads see the buffer and every sealed batch until the transaction runs.
- **Lazy metadata.** A member whose metadata tier is behind its core tier marks it not loaded at restore. The member's alarm applies the stored log to it, 500 entries per alarm, then marks it loaded. Core requests never wait. A command that needs the metadata tier while it loads is refused by `RepoScratch`.

### Snapshot hosting

`workers/site/src/snap.rs`, all on the member's alarm, armed 30 s after a member applies entries and every 20 ms while a job runs:

- **Writer.** `snapshot_writer(to, placement, leader)` picks the Full follower at `to / SNAPSHOT_EVERY` modulo the Full followers. Only that member reads the log's byte count for `snapshot_due`, at most once a minute. A job starts at `SNAPSHOT_EVERY` entries, `SNAPSHOT_BYTES` of log, or a new day.
- **Batches.** `SnapshotJob::begin` marks the tier's `to` and keeps `snap_sum`. Each alarm then reads 500-row frames through `snap_undo`, with a yield between reads, so Raft inputs run between them. A batch stops at 2 s or one 8 MiB part. `apply` never pauses.
- **Upload.** Each part goes to a multipart upload in two buckets, this site's and the next site's, under `R2Key::Snapshot { chain, tier, from: 0, to, digest }`. The digest is known at `begin`. The cursor, the upload ids and parts, and the unsent bytes (1 MiB rows of `snap_buf`) are stored with each part, so a reset object resumes.
- **Commit.** After the trailer, the writer asks the leader (`/snapshot_commit`) to HEAD both copies and record `Archived` with the key. When that entry applies, the writer runs `end_snapshot`.
- **Retention.** Applying a snapshot's `Archived` drops the `snapshot` rows `snapshots_to_keep` no longer keeps, one per tier at or below every index inside `RESTORE_WINDOW`. The writer then deletes those objects from both buckets.
- **Not built.** `InstallSnapshot`, for a member whose next entry the leader has archived. `crates/fold` has `restore` and `FrameCheck`, but no host path streams a snapshot into a fresh member. `build/BUILD-PLAN.md` puts the protocol in M3-WP5 (BF-29). Also not built: the log's own `R2Key::Archive` objects. The fold audit's replay therefore covers only chains whose stored log still starts at 1.

### The fold audit

`workers/site/src/fold_audit.rs`, run on the site's Cron Trigger after the sweeper:

- It takes 4 repository chains this site homes per run, after a cursor kept in the Registry object `fold_audit`.
- The chain's leader replays its stored log from index 1, at most 20,000 entries, into a fold in memory (`/fold {"replay": true}`).
- Each voter reports its own digest at that index from its digest ring (`/fold {"at": index}`), and `fold_audit_compare` compares them. Any mismatch is logged with `console_error` as an alarm.
- A member behind the index, or still loading, is listed but not compared.

On the final deploy, the only audit run was over `repo:196608`, whose log is empty, so it compared index 0 and proves only that the cron path runs. An earlier run found voter names missing, which `2ba89d5` fixed. The audit has not been run on a chain with a long log since, because both long-log groups were wedged (see "The defect").

## Data size

DESIGN, "Scale", "Member storage": "the largest repository's metadata tier (200,000 issues, 2 million comments) about 4 GB [guess]".

- **Seeding.** `/bench_seed` writes comment rows of 1,800 bytes of word text, from a 4,096-word vocabulary, into the metadata tier at an index at or below `applied`. It keeps the tier's row-hash sum, so `fold_digest` stays correct. Each call writes 10,000 rows in one transaction.
- **Speed.** Per member, the mean call took 4.2 s (apac), 6.1 s (enam), and 9.4 s (weur), and the slowest call 23 s. That is about 1,000 to 2,400 rows/s. 2.2 M rows took about 25 minutes per member, all three in parallel.
- **Bytes.** Each row cost 2,997 bytes of member SQLite, measured as `databaseSize` growth between 1,000 rows and 1.4 M rows. That covers the row, its key, the FTS5 postings, and the B-tree. So 4 GB is about 1.33 M of these comments, not 2 M. The design's 2 M comments at 4 GB implies about 2 KB per comment with its index.
- **Ceiling.** `repo:196627`: 2.2 M rows on all three members. `databaseSize` passed 4,294,967,295 at about 1.43 M rows and was reported as exactly 4,294,967,295 after that. `workers-rs` 0.8.7 reads it as a `u32`. The true size at 2.2 M rows is about 6.6 GB by the per-row rate [inference]. `fold_digest` matched on all three members (`27469d27…`), checked at index 134.
- **BF-20 group.** `repo:196628`: 1,330,000 rows, 3,986,579,456 bytes of member SQLite on each member, with `fold_digest` `a0aa5dc8…` on sites 1 and 2. Site 0 had applied one more entry, so it is compared through its digest ring at index 2 below.

So a 4 GB tier fits in one Durable Object and can be seeded. The limit that bit was not size (10 GB per object; `SQLITE_FULL` never appeared). It was what happens after a member restores; see "The defect".

## Load

The load runs on the leader (`/bench_seed {"load": ...}`). It sends one single-ref push per command, each its own `commit` input, paced at the offered rate, for 30 s. Every 3,000th command the leader also records one entry of 500 `Edited` events on seeded comments, so the metadata tier changes under a running snapshot and `snap_undo` is exercised.

"Apply p99" is measured on every member as two numbers:

- **Apply step.** The synchronous time of the input that applied the entries, from `applySteps`. It was 0 ms for every one of the 582 applying inputs in the table below, with and without a snapshot. That means under 1 ms, because `Date.now()` does not advance inside a synchronous step on Workers.
- **Entry age at apply.** Apply time minus the entry's `at`, from `applyMs`. On the leader this is the commit round. On a follower it includes the far follower's round trip and the AppendEntries queue.

### BF-20: baseline against snapshot

The snapshot ran on `repo:196628` (3.99 GB tier). The leader was site 0 (enam) and the writer site 1 (weur). The writer started the job by itself at 20:29:10Z: a tier with no snapshot is due, and the alarm ran 30 s after the first applied entry. It uploaded the last part at 20:47:36Z (`snap_log` `beganMs` to `uploaded`). So every `repo:196628` run after `pre150a` ran during the snapshot. The later forced start found the job already running.

The no-snapshot baseline is therefore `pre150a` plus the `b150*` and `b300*` runs on `repo:196627`. Those ran on the same install, with the same leader site, the same driver, and a 2.2 M-row seeded tier, before any snapshot of that tier began.

| Phase | Runs | Commands ok | Leader entry age p50 / p99 ms (n) | Follower entry age p50 / p99 ms (n) | Apply step max (inputs) |
| --- | --- | --- | --- | --- | --- |
| No snapshot, 150/s | `b150a`, `b150b`, `b150c`, `pre150a` | 18,000 / 18,000 | 377 / 1,462 (61) | 2,704 / 41,534 (106) | 0 ms (167) |
| Snapshot running, 150/s | `pre150b`, `snap150-1`, `-3`, `-5`, `-7` | 22,500 / 22,500 | 319 / 1,487 (76) | 2,519 / 8,880 (119) | 0 ms (195) |
| No snapshot, 300/s | `b300a`, `b300b`, `b300c` | 20,358 / 27,000 | 804 / 1,468 (31) | 3,737 / 34,624 (39) | 0 ms (70) |
| Snapshot running, 300/s | `pre300a`, `pre300b`, `snap300-2`, `-8` | 36,000 / 36,000 | 561 / 1,902 (55) | 3,389 / 6,943 (95) | 0 ms (150) |

- **Apply step.** Under 1 ms on every member in every input, with and without a snapshot. This is the cost `apply` itself pays, and the snapshot added nothing measurable to it. On this measure, apply p99 is within 10%.
- **Entry age at 150/s.** The leader's p99 was 1,487 ms against 1,462 ms, +1.7%, and its p50 was lower.
- **Entry age at 300/s.** The leader's p99 was 1,902 ms against 1,468 ms, +30%, but its p50 was 30% lower (561 against 804 ms). The baseline lost its leader in 2 of 3 runs, which cut those runs' tails out of the sample (6,642 commands refused `notLeader`). With 31 and 55 entries, each p99 is the single largest sample. So the 300/s comparison neither passes nor fails cleanly. The one sample that sets it is `pre300a`'s first entries, while the snapshot's first batches ran.
- **Followers.** Their tail was better during the snapshot. The baselines' 34 to 42 s are single entries that waited out a missed AppendEntries.
- Site 2 (apac) applied nothing from `snap150-5` on. The leader's AppendEntries to it held entries 148 to 177 (1.46 MB) and missed `CALL_DEADLINE` every time, after site 2's restore failed. That is the defect below.
- `snap300-4`, `-6`, `snap150-9` and later have no data. The driver's stats call came back empty during a 30 s egress-proxy outage on the orb (curl 502 from `172.28.0.25:3128` for the `graft-m1-wp7-edge` host only), or the group had stopped answering.

### The snapshot

From `snap_log` on the writer, and `snapcheck` on the R2 object in `graft-m1-packs-b`:

| | |
| --- | --- |
| `to` | 18 (`toAt` 1793737745595) |
| Rows | 1,330,000 |
| Bytes | 2,429,353,738 in 2,661 frames |
| Batches | 290, the worst 10,768 ms (by batch 116 the sum was 432.7 s) |
| Wall | 18 min 26 s from `begin` to the last part, about 1,200 rows/s |
| Key | `snapshot/repo:196628/metadata/00000000000000000000-00000000000000000018-5c3b76f4…9b6f` |
| `snapcheck metadata` | `{"ok": true, "to": 18, "rows": 1330000, "digest": "5c3b76f4…9b6f"}`. The stream's row-hash sum gives the trailer's digest, and the trailer's digest is the key's |

- The key's digest is `SnapshotJob::want`, the `fold_digest` at `to` from the sum `apply` kept when the job began.
- A refold of every row read through `snap_undo` produced the same digest. So the snapshot equals a refold at `to`, while 196 entries applied over it (index 18 to 214). The load submitted 14 entries of 500 `Edited` events on seeded comments in that time, so rows the snapshot read had changed after `to` and came from `snap_undo` [inference: the job does not count undo reads].
- The `/snapshot_commit` that would record `Archived` never answered, because the leader had stopped. So `end_snapshot` and retention did not run for this snapshot. On an earlier deploy, both tiers of `repo:196627` were snapshotted at index 22, committed, and verified the same way, at a few rows each.
- The worst batch, 10.8 s, is far past the 2 s `BATCH_MS` stop. One 500-row read through `snap_undo` merges two scans and can take seconds on a 4 GB tier. The member still answered Raft inputs between reads [inference: no apply step or round on the writer grew during the snapshot].

## The defect

The gate fails on request limits, and the groups that hit them stopped serving. The root cause is not established. What is measured, from `raw/tails-final/` and `/audit`:

- **When.** It happened twice. `repo:196627` broke at 17:04Z, just after a forced metadata snapshot began on site 1, with 2.2 M seeded rows per member and about 63,000 commands served in the session. `repo:196628` broke between 20:39Z and 20:49Z, during its snapshot, at about 80,000 commands. In both cases a member's isolate was reset first (a code update, an eviction, or `exceededMemory` on a `/read`).
- **Restore errors.** The reset member's restore then threw `Critical error` inside `SqlCursorRawIterator::next` (`Uint8Array::copy_to`), called from `Host::restore`. Its later requests hit `exceededMemory` 4 times on `repo:196628`.
- **Request rows.** At that point each member held 62,932 to 90,048 `request` rows (`/audit` `requestRows`), all of which `restore` loads into `GroupFold`, for the default 48 h `REQUEST_RETENTION`.
- **The hang.** Afterwards, on every member of the group, every POST hung for its full deadline with 0 to 1 ms of CPU, while GET routes that finish in their first poll kept answering. A throwaway build with `console_log` probes showed a POST reaching `Routes::fetch` and the line before its first real `await` (`faults::site_down`, which can fetch the `faults` Registry), and never logging again. Bounding that fetch at 2 s did not help. The members stayed this way across 4 redeploys and an object reset, so whatever holds them is restored from storage.
- **The group.** With no member answering POST, the others stepped down on `CommitErrors`, and the group had no leader.

Two hypotheses fit, and neither is tested:

1. **Restore memory.** Every `request` row is held in memory [inference]. M1 found the same class of failure: `tests/m1/results/m1-wp2.md`, "the 24-hour run", "the `oom-probe` … reset the leader object at about 387,000 request rows". The lin install runs with `GRAFTYARD_REQUEST_RETENTION_S` = 900 for that reason. The BF-20 install ran with the 48 h default.
2. **A stuck executor.** A task that dies inside the isolate leaves `wasm-bindgen-futures`' queue unable to run any task that yields [guess]. That would explain why every awaiting request stalls at about 0 CPU while synchronous ones answer. The tails show `memory access out of bounds` and `Cannot invoke closure from previous WASM instance` on these objects.

A retry with the 120 s retention override (deploys 12 and 13) could not test hypothesis 1. The `names` chains' restore check refused their stored rows under the new retention ("request rows differ from the log applied through Index(755)"), and identity went down. Deploy 15 restored the default.

So BF-20 measured the hosting work, and the group it ran in did not stay up. The next step is to reproduce a member restore at 80,000 request rows on a fresh group, with the retention override from the first deploy, and see which hypothesis holds. That is `crates/raft` and `workers/site` restore work, outside M2-WP1's package.

## Design sentences this contradicts or leaves unproven

- DESIGN, "Scale", "Member storage": "the largest repository's metadata tier (200,000 issues, 2 million comments) about 4 GB [guess]". Measured at 2,997 bytes per 1,800-byte comment with FTS5, 4 GB holds about 1.33 M comments. 2 M comments with FTS5 would be about 6 GB.
- DESIGN, Shape, `REQUEST_RETENTION` = 48 h, with "Per-group core-tier fold table `request(id, index, events)` … kept REQUEST_RETENTION after the entry's `at`". The hosted `GroupFold` holds every row in memory, and a group at the gate's 300 commands/s would hold 52 M rows over 48 h. M1 measured a reset at about 387,000 rows. Here a restore failed at 63,000 to 90,000 rows, though with a 4 GB tier beside them, and with the cause not proven.
- BF-20's "under 300 commands/s". Before the snapshot, the group carried 300/s in 1 of 3 runs. Earlier development runs on the same code carried 2,003, 2,798, and 7,485 of 9,000 (`s300c` to `s300e`). In `s300e`, `b300a`, and `b300b`, AppendEntries of 70 to 160 KB took 0.5 to 1.8 s to the far followers. They missed the 1 s `CALL_DEADLINE`, and the leader stepped down on `CommitErrors` (`raw/*.stats*.json`, `peers`). This matches M0's B0-02, "Between pass and fail: 159 to 280 commands/s at p99 750 to 1,288 ms".
- `SNAPSHOT_BYTES` = 256 MiB of log and `SNAPSHOT_EVERY` = 100,000 entries both assume a writer can read the whole tier in one cadence. At 1,200 rows/s through `snap_undo`, the 4 GB tier took 18.4 minutes. That is well inside a day. But the 2 s `BATCH_MS` budget [guess] did not hold at 4 GB: the first 116 batches averaged 3.7 s and the worst took 10.8 s.

## Checks

On the worktree at `2ba89d5`, plus this file:

- `cargo fmt --check`: ok.
- `cargo clippy --workspace --all-targets -- -D warnings`: ok.
- wasm32 clippy with `-D warnings`, for `graftyard-site` and `graftyard-edge`, with and without `test-faults`: all 4 ok.
- `cargo test --workspace`: every suite `ok` (`raw/checks2.log`).
- Long sweep, `RAFT_SEEDS=2000 RAFT_STEPS=10000`: 1 passed in 354 s (`raw/sweep.log`). It ran at `7bc7874`, and `crates/raft` has not changed since.
- Porcupine: HARSH 156,723 operations Ok, GRAY 2,226,280 operations Ok (`raw/porc2-*.json`). This used the checker with the `-seg` fix from `f966c04` on `main`, built outside the worktree, at `-seg 2000`. The worktree's own `tests/m0/porcupine`, from `db8535e`, still has the `-seg` bug and reports Illegal on the same histories (`raw/porc-*.json`).
- Product-build greps: site 0 of 42 needles present, and 42 of 42 in the `test-faults` build. Edge 0 of 20.

## What is deployed

`graft-m1-site-a/b/c` run the `test-faults` build of `2ba89d5`, deployed at 19:47Z (deploy 15, `raw/deploy15.log`), with the default `REQUEST_RETENTION`. `repo:196627` and `repo:196628` are wedged as described. `repo:196608` and the identity chains answer. `graft-m1-lin-*` was not touched.

## Reproduce

The driver scripts are in `tests/m2/bf20/` and expect to run from `/tmp/bf20`, with results in `/tmp/bf20/raw`. They call the `graft-m1-wp7-edge` raw member route with the bearer token in `tests/m0/.m0-token`, and the bed push route with the seeded writer's token in `~/.graft-bf20/tokens.txt`.

- `seed.py ROWS 10000`, with `CH`, `WRITTEN`, and `PFX`, seeds every member alike and resumes from its progress files.
- `bf20b.sh` runs the baselines, forces the writer's snapshot, and alternates 150/s and 300/s runs until the snapshot commits.
- `sum2.py` summarizes runs.
- `tail.py` keeps a `wrangler tail` per site.
- Verify a snapshot with `wrangler r2 object get <bucket>/<key> --remote --pipe | snapcheck metadata`, where `snapcheck` is `crates/git/examples/snapcheck.rs`.

`raw/` (not committed) holds every run's load answer and per-member stats, the seed progress files, the tails, and the deploy logs.
