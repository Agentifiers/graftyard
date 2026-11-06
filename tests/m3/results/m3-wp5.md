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
