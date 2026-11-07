# M3-WP5: learners and snapshot install, the pure half

BUILD-PLAN M3-WP5 asks for `MemberKind::Learner` with its own name slot, `LEARNER_AT`, `MAX_LEARNERS`, `place_learners`, learner read-index, and `InstallSnapshot` for a member whose next entry the leader has archived, which learners and `ReplaceMember` members also use (`design/DESIGN.md`, Shape, "Sites and placement", "The read path", "Packs"). This round builds the pure half and runs BF-28 and BF-29 in memory on real raft members. Hosting is not built: `place_learners`'s `graft-ops` cron, the site host's install path, and the B0-18 rerun. Nothing was deployed.

## What was built

- **Placement** (`crates/types/src/placement.rs`). `LEARNER_AT` (500 req/s), `MAX_LEARNERS` (16), `LEARNER_READS` (1,000 req/s per learner, from DESIGN's Scale row), `GroupLoad`, and `place_learners`. Below `LEARNER_AT` total reads, `place_learners` returns no region. Above it, each region with reads gets `ceil(rate / LEARNER_READS)` learners, handed out one round at a time from the busiest region, up to 16.
  - A learner's object is `{chain}#{gen}#l{slot}`, as before. It now also has an address, `Member::addr()`. A voter's address is its site. Learner slot k's address is `SiteId(192 + k)`, above every site id (sites are below 64).
  - `Placement::new` refuses a repeated or out-of-range learner slot (`SameSlot`). It also refuses two members that would be one object on one site (`SameObject`).
- **Raft** (`crates/raft`).
  - *Learners.* `quorum::learners` lists a config's learner addresses, and the leader replicates to voters plus learners (`targets()`). Votes, leases, commits, transfer targets, `STEP_DOWN`, probes, and the refusal hand-off still use voters only. The quorum rule counts only the config's voters, so a learner's ack never commits anything. A learner's `campaign` answers `MayNotLead`, and `may_take_lead` is false for it.
  - *Read index.* `Member::read_index` serves a read on a follower or learner. The member asks the leader with `Message::ReadIndex`. The leader grants it through the same lease or round as its own read, and answers `ReadIndexRes` with the index and that entry's digest. The member grants the read once it has applied that index; if its own entry there has the same digest, it commits through it. Reads are batched: a follower sends at most one batch every 10 ms, a learner one batch in flight and at most one every 50 ms (`LEARNER_READ_EVERY`). `fresh` accepts these grants.
  - *Install.* The leader's host calls `Member::set_install(InstallSource)` once its storage drops the entries at or below `through`. The source names the tier snapshot keys, the archive keys, `through` with its term and digest, and the committed config and id block. `send_append` sends `Message::Install` to a member whose next entry is at or below `through`. The member puts an `InstallWanted` in `Out::install` and does not answer the round.
  - The host loads the objects into a `GroupFold` and calls `Member::install`. It checks the fold is at `through` with the named config and id block, drops the member's log, sets its base to `through`, and emits `Write::Install(base)`. Only then does it answer the round with an ok `AppendRes` at `through`, so the member acks nothing before it has applied `through`. Repeats are idempotent: a member that already holds `through` answers ok.
- **Fold** (`crates/fold/src/snapshot.rs`). `Restore` loads a snapshot one frame at a time, and `restore` now runs on it. `refold_segment` restores a tier's snapshot into an empty store, then applies a log tail, holding one frame and one entry at a time. It returns the tier's `fold_digest`. This is F48's audit core (see "F48").
- **Hosts.** `workers/site/src/host.rs` gets three names for the new messages in `Call::of`, and an `unreachable!` arm for `Write::Install`, which that host never produces because it neither sets an install source nor calls `install`. `tests/m1/wp6/bed` and `crates/git/tests/push.rs` get the same arm.

## The gates

Both gates run in `crates/fold/tests/install.rs` on real `graftyard_raft::Member`s over a two-tier fold built from `crates/fold` (`apply`, `fold_digest`, `SnapshotJob`, `restore`). The host is a model of the site host:
- R2 is a map of objects. Snapshots are `SnapshotJob` frame streams, and archives are the entries in the raft codec.
- A leader's storage forgets the entries an archive covers, so `Out::catchup` below the boundary goes unserved, as on the site host after `Host::truncate`.
- An install restores both tiers from their snapshots. It then rebuilds the request rows of the archived entries still inside retention, and applies the archived entries above the snapshot.
- Members keep a log window of 4 applied entries and request rows for 60 s.

### BF-28: distinct objects. Pass.

The run, from `--nocapture`:

| Member | Address | Site | Object |
| --- | --- | --- | --- |
| voter f0 | 0 | 0 | `repo:7#1#f0` |
| voter f1 | 1 | 1 | `repo:7#1#f1` |
| voter f2 | 2 | 2 | `repo:7#1#f2` |
| learner l0, on its voter's site | 192 | 0 | `repo:7#2#l0` |
| learner l1 | 193 | 1 | `repo:7#2#l1` |
| learner l2, on the same site as l1 | 194 | 1 | `repo:7#2#l2` |

- Six members, six objects, six addresses. The host refuses to create a member whose (site, `do_name`) or address another member already has.
- The learners joined through one config change, a `Normal` placement with three learners. The leader had archived its log, so each of the three joined by `InstallSnapshot`.
- At the end each learner held the voters' tail and both tiers' digests at the common applied index. Each refolds to its kept digest.
- **Reads.** Each learner read core key 50 through `read_index` right after the leader committed 777 there, and saw 777.
- **No votes.** With both followers cut off, the leader appended an entry that all three learners stored, and it did not commit. It committed once the followers heard the leader again. A learner's `campaign` answered `MayNotLead`.
- **Read-index rate** (`learner_read_index_keeps_one_in_flight_and_a_50_ms_floor`). 40 reads arriving 2 ms apart went out as 2 batches 50 ms apart from a learner, and as 9 batches 10 ms apart from a follower. All 40 were granted on each.
- **Naming checks** (`crates/types`, `learners_beside_a_voter_and_on_one_site_are_distinct_objects`): the same layout gives distinct names and addresses, and `Placement::new` refuses a repeated or out-of-range slot.

### BF-29: offline past the archive boundary. Pass.

Follower C goes down at index 7. The leader commits to 46, follower B snapshots both tiers at 46, and the leader commits to 77. The leader then archives entries 17..=77 and forgets them; 17 is the first entry inside request retention. It commits to 90, and its in-memory base reaches 85.

C restarts. The leader sends `Install`, and C installs once. Before the install, every ack C sent named an index of at most 7. At index 89, the last all three had applied:

| Member | Core `fold_digest` | Metadata `fold_digest` |
| --- | --- | --- |
| A (leader) | `54b30f14825b` | `be167aab33ac` |
| B | `54b30f14825b` | `be167aab33ac` |
| C (installed) | `54b30f14825b` | `be167aab33ac` |

- C holds the leader's last entry (90, same digest).
- C holds the request rows of all 31 entries between the snapshot and the boundary that the leader still keeps; it can only have them from the archives.
- C's request rows, config, and both tiers' refolds match the leader's.
- A later write commits on C.

## Planted bugs

`tests/m3/wp5/plant.sh` applies each bug alone to a clean tree, runs the types placement tests and `install.rs`, and reverts it. All nine were caught:

| Plant | Gate | Caught by |
| --- | --- | --- |
| `do_name` without the kind letter | BF-28 | 3 placement tests |
| A learner addressed by its site | BF-28 | 1 placement test, BF-28, read-index rate |
| The quorum counts every ack, not the config's voters | BF-28 | BF-28 ("learners' acks made no commit") |
| A learner campaigns | BF-28 | BF-28 |
| The leader never sends `Install` | BF-29 | BF-29, BF-28 (stuck) |
| The member acks the install's round before installing | BF-29 | BF-29 ("acked nothing past where it left") |
| The host skips the archives | BF-29 | BF-29, BF-28 |
| The installed base's digest is wrong | BF-29 | BF-29, BF-28 (the next append does not chain) |
| `Restore` drops the snapshot's rows | BF-29, F48 | BF-29, BF-28, F48 |

## F48: a fold audit over `REPLAY_BYTES`

**Proposal, built in its pure half.** Refold from the newest snapshot, not from genesis, into scratch storage, and compare by `fold_digest`.

- **What the audit does.** For a sampled chain, the leader picks the tier's newest committed snapshot at or below the audited index, `from`. A host-side job streams the snapshot's frames from R2 into empty scratch SQLite tables with the tier's DDL (`Restore`, one frame per batch). It then applies the stored log tail `from+1..=to` with the same `apply`, one entry at a time (`refold_segment`). The resulting `fold_digest` is compared with every member's `digest_at(to)` through `fold_audit_compare`, as today.
- **Why memory stays bounded.** Memory is one frame and one entry, not the fold. The tail is at most `SNAPSHOT_EVERY` entries or `SNAPSHOT_BYTES` of log, because a snapshot is due by then.
- **Chaining.** A refold from snapshot k to snapshot k+1's `to` must reach k+1's trailer digest. That checks every archived range against the snapshots on either side, so a chain whose log is past R2 archiving is still audited segment by segment.
- **What it does not cover.** The first snapshot itself is trusted. Its rows were checked against the writer's running sum when written (`FrameCheck`), and the member that wrote it was audited at the time.
- **Test** (`f48_a_refold_from_a_snapshot_plus_the_log_tail_matches_every_member`). For both tiers:
  - A refold from the snapshot at 21 plus the tail matches all three voters' `digest_at` of the last common index.
  - A refold from 21 to the next snapshot reaches that snapshot's trailer digest.
  - A dropped entry is refused, and a changed entry gives a different digest.
- **Not built (hosted).** The site's `fold_audit` cron job that runs it on alarm-driven batches into scratch tables (`workers/site/src/fold_audit.rs`, `snap.rs::replay`). It is the same machinery as the install host's loader, so both should share one alarm-driven `Restore` loop. Until then `replay` keeps skipping chains over `REPLAY_BYTES`.

## Checks

At `54d7719`, except where a later commit is named:

- `cargo fmt --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- wasm32 clippy of `workers/site` and `workers/edge`, with and without `test-faults`: clean.
- `cargo test --workspace`: all pass.
- `./tests/m3/wp5/plant.sh`: 9 of 9 plants caught.
- The long sweep and Porcupine: see "Sweep and Porcupine" below.

### Sweep and Porcupine

Run at `54d7719` as an orb service, with the recipe of the F47 checks: `RAFT_SEEDS=2000 RAFT_STEPS=10000 RAFT_HISTORY=… cargo test --release -p graftyard-raft --test sim -- --ignored`, then `tests/m0/porcupine` with `-seg 2000` on each history. The later commits change only tests, so the result holds at HEAD.

- Long sweep: `long_sweep ... ok` in 336.8 s (2,000 seeds of 10,000 steps).
- Porcupine, HARSH history: `{"operations":156723,"noReplyWrites":27735,"droppedFailedReads":3343,"segments":6000,"result":"Ok"}`.
- Porcupine, GRAY history: `{"operations":2226280,"noReplyWrites":209,"droppedFailedReads":11,"segments":6000,"result":"Ok"}`.

The sweep's simulator (`crates/raft/tests/common/sim.rs`) runs three voters with no learners and no install source. It shows the learner and install changes leave the voter protocol, which is everything the simulator drives, unchanged under its faults. Learner replication, learner reads, and install are covered only by `install.rs`'s deterministic runs, not by the randomized sweep [inference: the sim's config has no learners and never calls `set_install`].

## Design sentences this contradicts or extends

- "`pub fn place_learners(load: &GroupLoad) -> ArrayVec<Region, 16>;`" (Shape, "Sites and placement"). The design never defines `GroupLoad`. Here it is per-region peak read rates, and `place_learners` hands out `ceil(rate / 1,000)` learners per region, busiest first, up to 16. The per-learner 1,000 req/s comes from the Scale row "1,000 req/s per learner and per leader". The rule itself is [inference].
- "`pub struct Envelope<E> { pub from: SiteId, pub to: SiteId, ... }`" in the crate, and the design's messages name sites. A learner beside a voter shares its `SiteId`, so a learner is addressed as `SiteId(192 + slot)` (`LEARNER_ADDR`). A host maps an address to the member's object through the config. This keeps every per-peer map in raft keyed by one id per member.
- "`pub struct InstallSnapshot { pub term: Term, pub core: R2Key, pub metadata: Option<R2Key>, pub archives: SmallVec<[R2Key; 8]>, pub through: Index }`" (Shape, "Packs"). Here it also carries:
  - the round fields an AppendEntries carries (`boot`, `round`, `sent_at`, `lead_seq`);
  - entry `through`'s term and digest, so the next append chains and the member can tell it already holds `through`;
  - the committed config and id block as of `through`, because no tier snapshot holds them.

  Keys are strings, because `crates/raft` cannot name `R2Key`.
- "It names the newest committed snapshot of each tier and the archive objects from there to the leader's first log entry" (Shape, "Packs"). The archives here start at the snapshot or at `REQUEST_RETENTION` before `through`, whichever is earlier. The installing member must rebuild the `request` rows inside retention, or a re-sent request whose entry is below the snapshot is decided twice. The tier snapshots hold no `request` table.
- "Each learner adds at most 20 read-index calls/s (one in flight, 50 ms floor)" (Scale). That holds, and a follower's 10 ms batches are 100/s at most. The leader also answers a learner's batch while its lease holds without a round. That is a read at the leader's applied index, so a learner read is linearizable through the leader's lease, as a leader read is [inference].
- BUILD-PLAN M2-WP1's note that `InstallSnapshot` is "not built". Its pure half is built now. The site host still refuses it (`Write::Install` is `unreachable!` there), so the note stays true for hosting.

## Commits

On branch `m3-wp5` from `main` at `5e364cf`, not pushed:

- `1b9c1d7` types: `LEARNER_AT`, `MAX_LEARNERS`, `place_learners`, learner addresses and slot checks.
- `867cab8` raft: learners replicate and read by read-index; `InstallSnapshot` for a member behind the archive boundary.
- `bb74d56` fold: streaming `Restore` and `refold_segment`; the BF-28, BF-29, and F48 gates on real raft members.
- `54d7719` the planted bugs, `tests/m3/wp5/plant.sh`.
- `6d5e866` `install.rs` prints each gate's numbers under `--nocapture`.
- `9374303` the raft codec test round-trips `Install`, `ReadIndex`, and `ReadIndexRes`.
- This file.

## 2026-11-06: the hosted half on `graft-m2b`

Branch `m3-wp5-hosted` from `main` at `c583053`. Every gate ran on the `graft-m2b` test install (`tests/m2/results/install-m2b.md`). `crates/raft` is unchanged, so the long sweep and Porcupine were not rerun.

### What was built

- **One loader** (`workers/site/src/load.rs`), shared by `InstallSnapshot` and the fold audit. A job stored in `load_job` runs one batch per alarm. Each batch either restores a tier from its R2 snapshot by range reads (`Restore::resume` from a stored `RestoreCursor`), or applies entries from the stored log (audit) or from archive objects (install). It writes into scratch tables (`tier::SCRATCH`, prefix `i`) beside the member's own. A batch ends after 2 s, 1 MB read, or 20,000 rows and entries. Its rows and the job's cursor commit in one transaction. A batch that began three times without committing is abandoned (`tries`), so a batch that kills its isolate cannot keep resetting the isolate's other members (F47).
- **Install.** The leader names an install source after it truncates (`Host::truncate` → `load::set_source`). The source holds the newest committed snapshot of each tier at or below the archive's end, the archives back to that or to `REQUEST_RETENTION`, and the config and id block. A member sent `InstallSnapshot` loads them. It rebuilds the `request` rows inside retention and calls `Member::install`. `Write::Install` drops the log, writes `log_base`, and swaps the scratch tables and `i_request` in, all in one transaction. The member then answers the leader's newest `InstallSnapshot` (`load_want`). A member that itself installed at the archive's end names a later source from `log_base` (`222fee6`).
- **Learner objects.** A learner joins with `/join` like a voter, without a registry row or RTT rows. Its address is `SiteId(192 + slot)`, which the host maps to `(site, do_name)` through the config (`peer_name`). `/read {"index":true}` serves read-index reads.
- **`place_learners`: the test-install equivalent, not a `graft-ops` cron.** `graft-ops` is not deployed on this account (`wrangler deployments list --name graft-ops` returns 10007). The leader's `/learners {"load": GroupLoad}` route runs `place_learners`. It joins each learner on the first non-spare site in its region under the next generation, and commits one `Normal` config change. The `graft-ops` cron would read Analytics Engine and call this route; that half is not built.
- **The F48 audit cron.** The site cron's `fold_audit` starts the leader's refold (`/fold {"audit":{"start":true}}`, daily or forced), then compares the last report with every voter's `digest_at`. A plain audit refolds each tier from its newest snapshot. A deep audit refolds the core tier from its oldest snapshot through the whole stored log and checks each later snapshot's trailer digest on the way (`segments`). `snap::replay` and its `REPLAY_BYTES` skip are gone. The report gives, per chain, the index, each tier's digest, batches, the slowest batch, bytes, rows, entries, ms, and `memMaxMb`, the isolate's wasm memory high-water mark.
- **Test-only routes** (`test-faults`): `/bench_seed {"archive":{from,to}}` has the leader put an archive object of its stored log in the first two record sites' buckets. The edge's `/_bed/archive` calls it, then records `Promoted` and `Archived` as before. `/read_bench` generates read-index reads inside a learner, as Milestone 0's driver did. `/fold {"requestsAt":n}` returns a count and digest of the `request` rows. Drivers: `tests/m3/wp5/bf29.sh` and `tests/m3/wp5/b0-18.ts`.

### BF-29 hosted: Pass

`tests/m3/wp5/bf29.sh 65539 2 30` on `repo:65539` (`m2b/bf29`, seeded for this). The pushes were real `git push`es through `graft-m2b-edge`. An earlier run on `repo:65537` could not archive: that repository's refs came from `/_bed/push`, so they name objects that do not exist, and the home shard's promotion refused them (`git update-ref ... nonexistent object`).

1. Site 2's member was held down by the edge's site fault from index 7.
2. 30 pushes committed. Site 1 snapshotted both tiers (core at 36, metadata at 37). The edge archived 1..=40: a promoted pack of 104 objects, then the archive object `archive/repo:65539/…01-…40-e0d35b71…` in buckets 0 and 1, then `Archived` at index 42.
3. The leader and site 1 truncated below 40 (`truncatedTo` 40). The fault was cleared.
4. Site 2 was sent `InstallSnapshot` and loaded both snapshots and the archive in **1 batch of 2,954 ms**: 20,267 bytes, 3 snapshot rows, 40 entries, 38 `request` rows kept, at 1.8 MB of wasm memory. It installed (`installs` 1, base 40) and caught up in 133 s, most of which was the driver's push loop.

At index 59, all three members' digests were equal:

| Member | Core `fold_digest` | Metadata `fold_digest` | `request` rows ≤ 59 |
| --- | --- | --- | --- |
| site 0 (leader) | `053b5f0b…` | `776e3f27…` | 57, `bc6e596d751d` |
| site 1 | `053b5f0b…` | `776e3f27…` | 57, `bc6e596d751d` |
| site 2 (installed) | `053b5f0b…` | `776e3f27…` | 57, `bc6e596d751d` |

The run also showed that a member which installed at 40 had no stored row at 40. Its own later truncate then found no install source ("no stored entry at through"). `222fee6` takes `through`'s term, digest, and time from `log_base` and the archive that ends there.

### BF-28 hosted: Pass

`node tests/m3/wp5/b0-18.ts --repo 65539 --place --load enam:6000,weur:5000,apac:5000`. `place_learners` gave 16 regions. The leader joined 16 learners, `repo:65539#2#l0` to `#l15`, at addresses 192 to 207: six on site 0 (the leader's site), and five each on sites 1 and 2, every one beside a voter. Config index 65.

There were 19 members, 19 distinct `(site, do_name)` objects, and 19 distinct boot ids. Each member's `/status` reported its own `doName`, and every learner was a follower. Fifteen learners caught up by `InstallSnapshot` (`installs` 1, base 40), because the leader's log starts at 40. The sixteenth, l0, joined first and reported `installs` 1 with commit 0 at the first read. By the next run it stood at commit 80 with base 41. The verdict held on all five later runs.

### B0-18 rerun: mixed, 2 of 6 runs pass

Each run: 16 learners × 1,000 read-index reads/s for 30 s, generated inside each learner (`/read_bench`), while the group took 10 pushes/s. Pass is leader under 700 req/s and learner p99 at most 1 s.

| Run | Reads/s | Failed reads | Leader req/s | Learner p50 (median) | Worst learner p99 | Verdict |
| --- | --- | --- | --- | --- | --- | --- |
| a, 10:19 | 16,000 | 0 | 172 | 162 ms | 2,435 ms | Fail |
| b, 10:22 | 16,000 | 0 | 184 | 161 ms | 727 ms | **Pass** |
| c, 10:23 | 14,280 | 51,594 | 195 | 157 ms | 1,312 ms | Fail |
| r1, 10:25 | 16,000 | 0 | 206 | 158 ms | 716 ms | **Pass** |
| r2, 10:26 | 16,000 | 0 | 204 | 159 ms | 1,207 ms | Fail |
| r3, 10:28 | 14,942 | 31,736 | 173 | 163 ms | 1,488 ms | Fail |

- **Leader load passes every time:** 172 to 206 req/s, against a 700 limit. `/raft` was 4,177 to 4,926 leader requests a run, and the rest were the pushes' `/commit`, `/outcome`, and `/read`. Milestone 0 measured 171 to 180.
- **The learner p99 is what fails.** The passing runs match Milestone 0's 683 to 743 ms. The failing runs are not a slower steady state. In two of them (c, r3), every learner failed 118 to 2,461 reads, about 1 to 3 s of reads each. That looks like a stretch where learners knew no leader, or the leader answered no read-index batch. Between runs c and r1 the leader moved from site 0 to site 1.
- **"Subrequest depth limit exceeded" does not explain it.** Workers Logs show bursts of this error on `/outcome`, `/outcome_by_request`, `/commit`, `/read`, and `/raft` (Workers and objects may nest 16 calls):
  - site a: 28 events at 10:19:30–10:19:49, and 100 or more at 10:24:00;
  - site b: 90 at 10:25:40, and 100 or more at 10:28:00;
  - site c: 10 at 10:19:40.

  These fall at or just after the ends of runs a, c, r1, and r3 (each summary's `at` is the run's end). But r1 passed with a burst, and r2 failed with none. Milestone 0's prototype saw the same error on `/probe` (`tests/m0/results/m0-wp2.md`). It is a host matter (`workers/site/src/host.rs`, where a `/raft` reply is stepped and new sends leave from inside the incoming call), but this run does not tie it to the p99.
- **Cause: not found.** The p99 failure needs a trace of one learner's read-index batches against the leader's `ReadIndexRes` times in a failing run, which I did not take. Until then, B0-18 on real members is unmet.

### F48: `repo:65536` refolded, peak wasm memory 64.9 MB

`repo:65536` (`m2b/bf20`) holds 5,790 entries and 107.7 MB of stored log, a 2.4 GB metadata tier, and a 31.6 MB core snapshot at 5,735.

| Run | What it refolded | Batches | Slowest batch | Read | Time | Peak wasm memory | Result |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Plain (09:11, build `aa6397d`…`ca6bd97`) | core from the snapshot at 5,735 (571,877 rows) plus 43 entries, to 5,778; metadata skipped (snapshot 2.4 GB > 256 MB) | 6 | 1,731 ms | 32.7 MB | 15.9 s | 111.3 MB | core `1ed53b29…` equals all 3 voters' |
| Deep, 4 MB batches (09:13, same build) | core from the snapshot at 2 through the log | — | — | — | — | — | isolate died 3 times ("Critical error … SqlCursorRawIterator … Uint8Array::copy_to", "Reinitializing Wasm application") |
| Deep, 1 MB batches (10:08, `222fee6`) | the same | 84 at the failure | 405 ms | 106.6 MB | 42 s | 64.9 MB | failed at the end: SQLITE_LOCKED dropping scratch tables |
| **Deep, 1 MB batches (10:16, `d6e350d`)** | core from the snapshot at 2 plus 5,788 entries, to 5,790 | **213** | **405 ms** | 269.5 MB | 57.7 s | **64.9 MB** | **core `7c66455d…` equals all 3 voters' `digest_at(5790)`. The refold reached the snapshot at 5,735's trailer digest `29483fa7…` on the way (`segments` ok)** |

- Peak wasm memory for the deep audit was **64.9 MB**; the isolate stood at 56.6 MB before it started. Workers Logs show no isolate reset or memory error on any site during the run.
- The plain run's 111.3 MB is the isolate's high-water mark, which a member restore had already set before the audit began. Linear memory never shrinks, so it is a ceiling for that audit, not its own use.
- Two fixes came from these runs:
  - `aa5e0f4`: 4 MB of log in one batch, held as rows and again as statements, took the isolate past 128 MB, so batches are 1 MB and abandoned after three deaths.
  - `d6e350d`: the audit's log cursor was stopped early, held the log table, and the closing `DROP TABLE` failed with SQLITE_LOCKED, so it now reads pages to the end.
- The bytes read (269.5 MB) are more than the log (107.7 MB). Each batch rereads its last page, because a page is read whole and only the entries that fit the batch are applied [inference].
- The metadata tier of `repo:65536` is still not refolded. Its 2.4 GB snapshot would double the member's 4 GB of storage in scratch tables (`AUDIT_SNAPSHOT_MAX`, 256 MB). Auditing a tier that large needs scratch space outside the member, for example a separate audit object.

### Checks

At `d6e350d`:

- `cargo fmt --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- wasm32 clippy for `workers/site` and `workers/edge`, with and without `test-faults`: clean.
- `cargo test --workspace`: all pass. This includes `a_restore_resumed_across_batches_from_its_stored_cursor_ends_at_the_trailers_digest` (`crates/fold/tests/install.rs`).
- Product-build greps: site 0 of 50 needles in the release build, edge 0 of 20. With `test-faults`, the edge has 20 of 20. The site has 49 of 50: the release build inlined `fail_next_read`, so its symbol name is gone, but the feature's strings `failRead` and `graftyard_no_such_table` are both present. The new test routes are present (`/read_bench`, `::archive_put`).
- `crates/raft` is unchanged from `main`, so the long sweep and Porcupine were not rerun.

No planted bugs were added for the hosted gates. The pure half's nine plants (`tests/m3/wp5/plant.sh`) cover the raft and fold logic the hosted gates run.

### What is deployed

All on `graft-m2b`, all `test-faults` builds:

- `graft-m2b-site-a`, `-b`, `-c` at `d6e350d` (versions `dbd0c14a`, `681df06d`, `fc37446a`; wasm `53c14f27…`), with git shard image `graft-m1-site-gitshard-git@sha256:aa3038ea…`.
- `graft-m2b-edge` at `0de6314` (version `097cf03d`). The edge did not change after that commit.
- Repository `m2b/bf29` (`repo:65539`) was seeded by `tests/m1/integ/seed.ts`. It holds 16 learners at generation 2.
- The edge's site fault is off.

### Design sentences this contradicts or extends

- "Daily `graft-ops` cron: none under LEARNER_AT, else ceil(rate / 1,000) per region, busiest first, up to 16" (Shape, "Sites and placement"). There is no `graft-ops` on the test account. The decision runs from the leader's `/learners` route, and the cron's Analytics Engine read is not built.
- "It names the newest committed snapshot of each tier and the archive objects from there to the leader's first log entry" (Shape, "Packs"). As in the pure half, the archives start at `REQUEST_RETENTION` before `through` when that is earlier. Here the source is also named by a member that installed, from `log_base`.
- `R2Key::Archive` objects have no stated format in the design. Here an archive object is `gyfarch\0`, version 1, then each stored log row in the raft codec behind a 4-byte big-endian length (`crates/fold/src/archive.rs`). It is not JSON, so an install applies the entries byte for byte as committed. The test route builds the object in memory, which bounds an archive at what one isolate holds. A product archiver needs a multipart put.
- "Refolds sampled chains from genesis on each release and daily" and "Until the site's cron runs it (M2-WP1), a chain whose log is over `REPLAY_BYTES` … is reported as skipped" (`design/SHARED.md`, "Folds", `fold_audit`). The cron runs it now and nothing is skipped for log size. A tier whose snapshot is over `AUDIT_SNAPSHOT_MAX` (256 MB [guess]) is still skipped, because its scratch copy would not fit beside it.
- A learner has no registry row. The design's registry rows are per voter, and it says nothing about learners there.
- B0-18 is restated in BUILD-PLAN as "Leader under 700 req/s, learner read p99 at most 1 s". On real members it passed 2 of 6 runs, for a cause not found, so the M3 gate "Learners rerun the Milestone 0 learner gate" is not met.

### Commits

On `m3-wp5-hosted`, not pushed:

- `aa6397d` fold: a resumable `Restore` cursor, `finish_rows`, and the archive object's row format.
- `0de6314` site: host `InstallSnapshot` and learners; one batched loader for install and the F48 fold audit.
- `ca6bd97` site: the loader yields between reads and answers the leader's newest `InstallSnapshot` for its source.
- `aa5e0f4` site: loader batches bounded by rows and 1 MB, and a batch whose isolate died three times is abandoned.
- `222fee6` site: an installed member names an install source from its `log_base`; the BF-29 hosted driver.
- `d6e350d` site: the audit reads the stored log in pages read to their end.
- This section and `tests/m3/wp5/b0-18.ts`.

## 2026-11-06 (evening): B0-18 traced on `graft-m2b`

Branch `b0-18`, with `main` (`856fc32`) merged at `9a83fa4`. Notes are added as the work goes.

### State before this round

- `graft-m2b-site-a`, `-b`, `-c` ran versions `dacc1247`, `f675d657`, `b4ae45f1` (12:10Z). That is the afternoon's `b0-18` build with the `/depth_left` gauge, made before main's 16:16 merge, with git shard image `aa3038ea…`. `graft-m2b-edge` ran `b2309a5a` (11:23Z).
- The afternoon's 11 runs (`/tmp/m3h/b0-18`, 11:27 to 15:23) all failed. The four with `--pushes 0` failed as well. That rules out the pushes as the cause.
- The `/depth_left` gauge poisons the object it measures. Learners `l0` and `l2`, the two learners the gauge polled, logged 22 to 30 "Subrequest depth limit exceeded" `readIndex` failures in each gauged run. Every other learner logged 0 to 2. A check at 21:00 on learner `l3` shows why. The gauge alone reached 14 hops. Started 1.2 s after a 14-hop `/depth_hop` chain on the same object, it reached 0. The chain had already ended. So the runtime charges an object's outbound calls to the newest request the object received, even after that request has finished (workerd `IoContext::getCurrentIncomingRequest`, "the newest request that hasn't already completed", which an actor keeps current while any task is pending). The gauge is not run during the runs below.

### Deployed for the trace (22:36Z)

- `91bf03e` adds a read-index trace to `/stats` on test-faults builds. Each member records every `ReadIndex` it sends and that call's end, every `ReadIndexRes` it steps (from its own call's answer, or from a call the leader made), every move of its applied index, and every wake of its 100 ms ticker that is 50 ms or more late. The leader records each `ReadIndex` it takes and each `ReadIndexRes` it sends, inside the asker's answer or as its own call. `read_bench {"samples": true}` returns every read's start, latency, and end for one learner (`b0-18.ts --trace-slot`, default 0).
- Site day with `--roll-shards`: `graft-m2b-site-a` `6024f7f3`, `-b` `a832c636`, `-c` `828f27f1`, wasm `c99472241ae96804…`. Image `graft-m1-site-gitshard-git@sha256:8fa9abd4…`, main's image since `ca15db8` (geometric repack). The shard roll did not log: the supervised service printed nothing after `wasm-opt`, though all three versions deployed. Two running shards (`git#0#net:65537`, `#65539`) run the new image, and the rest are stopped.

### Trace runs 22:38 to 22:50Z: 12 runs, 12 fail

On `91bf03e`, 16 learners × 1,000 reads/s for 30 s, with 10 pushes/s (raw data in `/tmp/b018/runs`, not committed). Every run failed on the learner p99, at 1,310 to 2,865 ms. Failed reads were 0 to 53,532 per run. Every run also had at least one `STEP_DOWN` `CommitErrors` trip on the leader, with each voter's calls 3 of 8 to 28 failed in the window. Failed reads come in 2 to 3 s blocks that start within a second of a trip.

Learner `l0` (site 0) in the first run (22:38:57, leader site 0, then 1, then 2): 297 batches. **None queued behind the one in flight.** Each was sent 50 ms after the last was answered, never sooner, so the 50 ms floor set the gap (p50 50 ms). The leader answered in its reply to the learner's own call, p50 33 ms and p99 404 ms. No batch was resent. Every read the learner served waited for nothing but its batch's answer. The read index was always at or below the learner's applied index when the answer arrived (wait-to-apply 0 ms for 293 of 297). Its 1,868 failed reads were all `notLeader`, at seconds 8 to 9 and 24 to 25:

| Offset | Event |
| --- | --- |
| 8,627 ms | batch 164 leaves for site 0 (the leader) |
| 8,693 | site 0's `STEP_DOWN` trips (`CommitErrors`, sites 1 and 2 each 3 of 24 failed), and it starts a transfer to site 1 |
| 8,709 | site 0 takes batch 164 while it transfers. A transferring leader serves no `fresh`, so the read waits |
| 9,019 | site 0 answers `None` (not leader) in its own call, 310 ms later. The learner's queued reads fail `notLeader` |
| 9,164 | batch 165 leaves for site 0 again, which still names itself. `None` again |
| 9,294 | batch 166 leaves for site 1, the new leader. Answered at 9,451 |

The failed calls that tripped `STEP_DOWN` were site 0's AppendEntries to its voters, and to every learner as well. They failed with "Subrequest depth limit exceeded" (`fetch`) at 6.2 to 8.9 s, and then 118 AppendEntries missed their 1 s deadline together at 9.7 s. The same pattern repeats at 24.3 s on site 1 and at 26.8 s on site 2. The slow reads near p99 (2.7 s) sit in the same windows.

So the p99 failure is not read-index queueing or apply wait. It is the leader stepping down, or holding, because its AppendEntries fail "Subrequest depth limit exceeded". Next: where the depth comes from.

### Where the depth comes from: a ratchet through leader and learner calls

`29c667a` models each member-to-member call's nesting depth. Every `/raft` call carries `x-graft-hop`, which is its sender's newest incoming hop plus one, and an outside request counts as hop 2. `/stats` counts calls by modelled hop. The site day at 22:58Z (`12c46948`, `3fb07064`, `c23d2b33` then `97a992b1`, `ba2e4cad`, `71fd582b`, wasm `b2afe73e…`) did not move the three voters. They ran the 22:36 build, with isolates from 22:36, until each was sent `/reset` at 23:02. So a site day does not restart a member that keeps receiving calls, and a member can keep running the previous version for minutes.

Three runs on the new build, all voters reset (23:03 to 23:06): all failed, at p99 1,592 to 2,715 ms. In the second run the leader's (site 2) calls by modelled hop went 3, 5, 7, 9 … 21, 23, 25, 26. Every call at hop 25 and 26 failed "Subrequest depth limit exceeded" (72 and 57), and 69 of 105 failed at hop 21. The model is not exact, since the runtime keeps a request current only until it completes, but it climbs by 2 per exchange.

The mechanism:

1. The runtime charges each outgoing call to the object's newest incoming request. The `/depth_left` check above shows this.
2. The leader's AppendEntries and its delayed `ReadIndexRes` are calls to each learner, so a learner's newest request is the leader's call, at depth d + 1.
3. `read_bench` generates reads inside one long request, so a learner gets no other request. Its next `ReadIndex` call leaves at d + 2.
4. 16 learners call the leader about 150 times a second, so the leader's newest request is almost always a learner's `ReadIndex`. Its next AppendEntries leave at d + 3, to the learners and to both voters.
5. At 16 the leader's calls fail. That includes its AppendEntries to the two voters, so `STEP_DOWN` trips on `CommitErrors` (3 or more of each voter's calls failed in 10 s). The leader transfers or holds. Learner reads fail `notLeader`, or wait for the next leader.
6. A push's `/commit`, an edge `/read`, or a tools-edge request is a new shallow request on the leader, and so is a driver call on a learner. Each one resets that object's depth. That is why runs pass or fail at random, and why the Workers Logs bursts do not line up with runs.

**The learner read path does not call back through lookup or the edge.** `read_bench` → `Member::read_index` → `ReadIndex` → `Host::send` → `call_peer` → the leader's `/raft`, over the member binding. The leader answers in the response, or later as its own `ReadIndexRes` call. The nesting is not one call chain. It builds up across separate calls, because each one inherits its object's newest incoming request.

**Test of the mechanism (23:31 to 23:33Z).** The same two runs, while `ping.mjs` sent every learner about 22 shallow `/ping`s a second from outside. Both passed: p99 762 ms and 620 ms, 0 failed reads, leader 198 and 199 req/s, no `STEP_DOWN`, no depth failure. The leader's calls stayed at modelled hops 3 to 8. Nothing else changed.

Milestone 0's prototype passed because its learners pulled entries inside their own read-index call. The leader never called a learner (`tests/m0/results/m0-wp2.md`, B0-18). M0-WP9 changed that so the leader feeds learners (`m0-wp9.md`, "Learners fed by the leader"), and that change closed the loop. In production, edge reads arriving at a learner would reset its depth, but a learner whose reads arrive more slowly than the leader's calls would still climb.

### The fix, at the cause: no call cycle between a leader and a learner

Each learner forms a two-way call cycle with the leader: `ReadIndex` calls one way, and AppendEntries plus delayed `ReadIndexRes` calls the other. That cycle is what lets the runtime's depth grow with no bound. The fix makes calls flow one way at a time, in the host's transport. `crates/raft` does not change, and neither do any of its messages.

- **Hold for a calling learner.** While a learner has a call in progress at this member, or called it in the last `CALL_DEADLINE`, every message for that learner is held. The held messages go in the response to the learner's next call, ahead of that call's own replies, so a learner applies the entries before it grants the reads. A learner that stops calling gets its held messages as calls from the 100 ms ticker. An idle learner, one serving only feed polls, is called as before.
- **Answer a read index in its own call.** A `ReadIndex` the leader cannot answer at once, because it needs a round, holds its response for up to 500 ms for the answer. Before, the answer went back as the leader's own call.

With both rules, the leader calls a learner only while that learner is not calling it. A learner's calls are then charged to its own newest outside request, and the leader's calls sit one hop above those.

### The fix as built: `06762a2`

- `workers/site/src/callers.rs` holds `Callers`, a per-learner record of calls in progress and the last call's time, with the messages held for each learner. `Host::send` passes every envelope through `Callers::route`. A learner with a call open here, or one that called within `QUIET` (1 s), gets the message held. Anything else leaves as a call (`Host::call`), as before. The 100 ms ticker sends what `Callers::due` returns: the held messages of a learner that has not called for `QUIET`. Voters are never held.
- `/raft` takes the held messages into the response to a learner's call, ahead of the call's own replies, after the output gate. A learner's `ReadIndex` that the leader could not answer at once waits in the call, up to `READ_INDEX_HOLD` (500 ms), for its `ReadIndexRes` to be held for it.
- The test `a_learner_that_calls_the_leader_is_never_called_back_so_depth_stays_bounded` models the runtime's rule: each call leaves at its sender's newest incoming depth plus one. It drives one leader and one learner for 30 s with `read_bench`'s pattern and no pushes. The old routing nests past 16, and the test asserts it does. With `Callers`, every call stays at depth 3. A second test covers holding, `QUIET`, and sending again.
- `crates/raft` is unchanged, so the long sweep and Porcupine do not apply.

### First gate runs on the fix (01:15 to 01:21Z): 4 of 6 pass, and the depth failures are gone

Deployed `06762a2` at 01:13Z: `graft-m2b-site-a` `442d7cfe` (first phase `c35e923f`), `-b` `3bbbc8a9` (`041b2aae`), `-c` `1fc03ca7`/`be45c2d5`, wasm `47c97bc8…`. All 19 members were then sent `/reset` so each one runs it.

| Run | Leader req/s | Worst learner p99 | Failed reads | Depth failures | `STEP_DOWN` | Deepest modelled hop | Verdict |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 01:15:55 | 244 | 851 ms | 0 | 0 | 0 | 6 | Pass |
| 01:16:56 | 248 | 564 ms | 0 | 0 | 0 | 4 | Pass |
| 01:17:58 | 249 | 775 ms | 0 | 0 | 0 | 4 | Pass |
| 01:19:00 | 251 | 1,097 ms (slot 8) | 0 | 0 | 0 | 4 | Fail |
| 01:20:03 | 246 | 603 ms | 0 | 0 | 0 | 4 | Pass |
| 01:21:04 | 247 | 1,729 ms (slot 0) | 0 | 0 | 0 | 6 | Fail |

The leader made 334 calls in the first run, 114 of them to learners, and none failed. In the failing runs, one learner each had one slow second (slot 8 at 13 s, max 1,290 ms; slot 0 at 7 s, max 2,028 ms). Every other learner's p99 was under 900 ms.

Slot 0 in the 01:21 run: batch 142 left at 7,087 ms and reached the leader at 9,058 ms. Its resend after `CALL_DEADLINE`, at 8,139 ms, also reached the leader at 9,059. Meanwhile the leader took 295 batches from other learners on time. Once `QUIET` had passed, the leader called slot 0 twice, at 8,230 and 8,330. Slot 0 applied the first at 8,309, but both answers took 825 and 751 ms. So for about 2 s, nothing left slot 0, and the learner's other work kept running. That matches its storage confirm stalling: the output gate holds every outgoing call and response until the writes are durable. Slot 0 had just applied an entry at 7,087. This is not yet measured, so the next build times every member's storage confirm.

### Second set on the fix, with confirm times (01:29 to 01:35Z): 4 of 6 pass

`1808695` adds each storage confirm's time to the trace. Deployed at 01:28Z (`6f9a1a83`, `219251d9`, `c936463d` / `9607dd62`; wasm `16638bcc…`), then all 19 members were reset.

| Run | Leader req/s | Worst learner p99 | Failed reads | Slowest storage confirm | Verdict |
| --- | --- | --- | --- | --- | --- |
| 01:30:16 | 240 | 723 ms | 0 | 302 ms | Pass |
| 01:31:18 | 239 | 749 ms | 0 | 355 ms | Pass |
| 01:32:20 | 246 | 782 ms | 0 | 407 ms | Pass |
| 01:33:23 | 244 | 1,382 ms (slot 8, site 2) | 0 | 244 ms | Fail |
| 01:34:25 | 237 | 1,009 ms (slot 8, site 2) | 0 | 378 ms | Fail |
| 01:35:28 | 236 | 951 ms | 0 | 522 ms | Pass |

There were no depth failures, no `STEP_DOWN`, and no failed reads in any of the 12 runs on the fix. Storage confirms are not the tail: the slowest was 522 ms, and slot 8's confirms were 31 to 50 ms around its slow second.

What remains is a single `ReadIndex` call, from a learner on site 2 (apac) to the leader on site 0 (enam), whose answer is lost or late. In the 01:33 run, slot 8's batch 42 left at 9,970 ms and never reached the leader. The leader recorded taking batches 41 (9,791) and 43, and nothing for 42 until the resend. `tick_remote` resent it at `CALL_DEADLINE` (1 s), and that resend reached the leader in 71 ms. The 1 s of reads queued behind the one batch in flight therefore waited 1 s more than their round trip, so the worst reads came to 1.4 to 1.6 s. Nothing else on the learner or the leader stalled: the leader took 15 to 20 batches every 100 ms the whole time. In the 01:35 run, 6 learners' calls all took 619 to 694 ms at 10.8 s, with most of the time on the reply's way back. That was one network event on the enam–apac path. Those runs fall inside the gate only because the lost call is rare.

That is the "batches queue behind the single in-flight one" case. Here it is hit by one call lost on the network, where before it was hit by every call being refused. The learner waits `CALL_DEADLINE` before it resends.
