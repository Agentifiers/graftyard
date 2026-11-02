# M1-WP5: git cache shards and the git container image (container side)

Run 2026-11-01 in the Milestone 0 account. The shard logic is in `crates/git`. `shard.rs` and `ingest.rs` are pure: `shards_for`, rendezvous, the start cap, the region check, the alarm renewal, the image map, `IngestCursor`, and the rehydrate, catch-up, and promotion plans. `host.rs` is the Workers side (feature `worker`): `ShardHost` drives the container through `getTcpPort(8080)` with `validate`, `upload_pack`, `rehydrate`, `catch_up`, `promote`, and `fsck`.

The image is `containers/git`. No site script hosts the shard class yet. The row runs on its own throwaway script, `graft-m1-wp5` (`tests/m1/wp5`), with a `GitShard` container class under the `durable_object` policy, three buckets (`graft-m1-wp5-a` ENAM, `-b` WEUR, `-c` APAC), and `Chain`, a stub chain leader.

## The image

| | |
| --- | --- |
| Base | `alpine:3.22` (3.22.7) |
| git | 2.56.0, upstream, built from `git-2.56.0.tar.xz` on mirrors.edge.kernel.org and checked against its published SHA-256 `26c56c29…89d3`. Built with its default Rust parts (Alpine's `cargo` 1.87 in the build stage) and `NO_REGEX=NeedsStartEnd`, because musl's regex lacks `REG_STARTEND`, as Alpine's own package also handles. Built without Perl, Python, Tcl, or gettext, then stripped |
| Server | `containers/git/server/main.go`, Go 1.23, static (`CGO_ENABLED=0`): `/health` (disk id, git version, `CLOUDFLARE_REGION`, `CLOUDFLARE_LOCATION`), `/init` (bare repository with reftable), `/validate`, `/ingest`, `/refs`, `/promote`, `/info-refs`, `/upload-pack`, `/fsck` |
| Size | 74.2 MB uncompressed (`docker images`), 23.9 MB as a gzipped `docker save` |
| Build | `docker build containers/git` (about 60 s on this orb). Wrangler builds the same Dockerfile on `wrangler deploy` from `tests/m1/wp5/wrangler.jsonc` (`images.git.dockerfile`) |
| Push | `wrangler deploy` pushes to the account registry. The image of record is `registry.cloudflare.com/<account>/graft-m1-wp5-gitshard-git:wrangler-mvx7saui`, digest `sha256:ee63672d0329b26e4ffb93258ebc75a963a97ad003ea84a49e9d36f1c49595e9`. The earlier tag `wrangler-mvx6y2my` (`sha256:08434e5b…`) lacks the `tmp_pack` cleanup below |

The docker daemon works in this orb, and the image was also tested locally with real git. Those tests covered validating a thin push against refs, refusing an unconnected tip (422), answering 409 for a thin pack whose base has not arrived, answering 422 for a checksum mismatch, checking that a promoted pack indexes on its own (complete), and cloning over protocol v2 and v0 through a smart-HTTP proxy.

## BF-18 on Cloudflare Containers

`tests/m1/wp5/bf18.sh` is the test driver. It stands in for the edge, whose receive-pack is M1-WP6. It builds real history in a local repository and sends each push as the pack `git pack-objects --revs --thin` makes against the refs the server holds, which is what receive-pack receives. The bed then validates it on the home shard (`index-pack --strict --fix-thin` into a quarantine, plus connectivity of the new tips), places it (`PackStore::place`), commits it (an append on the stub, with an `expect` check per ref), and puts the `Committed` markers. Shards are `standard-2`. `0-0` is the home shard (ENAM hint), and `1-0` is the second warm site's shard (WEUR hint).

The commands were `cd tests/m1/wp5 && BF18_NET=5000003 worker-build --release && npx --prefix ../../m0 wrangler deploy`, then `WP5_URL=https://graft-m1-wp5.<sub>.workers.dev WP5_TOKEN=… tests/m1/wp5/bf18.sh /tmp/bf18c`. The output is in `raw/bf18-net5000003.log` and `.jsonl`.

| Step | What happened (network 5000003) |
| --- | --- |
| Pushes 1 to 8 | Six inline packs (344 to 962 bytes), one stored pack at index 7 (1,500,719 bytes, a 1.5 MB random blob, placed in three buckets and marked), and one more inline pack. Both shards caught up. `1-0`'s first call rehydrated it from 1 marker and 7 inline packs |
| Archive 1 to 8 | The home shard packed the objects of the range's 7 inline packs into one complete pack (42 objects, 3,474 bytes). The bed validated it, placed it as stored with `inline_max: 0`, re-put the marker of the range's stored pack, committed `Promoted` (index 9), and marked it. It then put the archive object (`archive/repo:5000003/…01-…08-38cad5e7…`) create-only in two buckets and committed `Archived` (index 10), which truncated entries 1 to 8 from the stub's log |
| Pushes 11 to 14 | Two inline packs, then a stored pack (1,200,638 bytes) on topic-a whose marker write was killed (index 13), then one more inline pack. `1-0` caught up from cursor 8 across the promotion and the truncated range: 5 entries, 4 packs, no rehydrate |
| Kill all shards | `destroy()` on both containers. `0-0`'s object kept its SQLite (cursor at 13), but no container ran |
| Rehydrate `0-0` | It found a new disk id and treated the stored cursor as describing a lost disk, so it ran a full rehydrate: 2 markers (packs 7 and 9), 3 stored packs (those two, plus pack 13 from the log after the promoted range), and 3 inline packs (11, 12, 14), 6 ingests, 2.71 MB. `git fsck --strict`: ok, garbage 0. The container started in ENAM (mia10) with 0 replacements |
| Pushes 15 to 17 | While `1-0` was down and behind |
| `1-0` | It found a new disk id and rehydrated: 2 markers, 3 stored and 6 inline packs, 9 ingests, in WEUR (cdg15). After one more push (18), it caught up from its cursor on the same container: 1 entry, 1 pack, no rehydrate. `git fsck --strict`: ok, garbage 0 |
| Clones | `git clone --mirror` from each shard over protocol v2 and v0 (4 clones): every one had all 5 refs equal to the stub's refs and passed `git fsck --strict`. A `--single-branch` clone of each of the 5 refs from `1-0` (5 clones) matched each tip and passed fsck |
| Fetch | After push 19, an incremental `git fetch` from `1-0` into the v2 clone moved `main` to the new tip |

The run reported 0 failures. Two more runs gave the same results on earlier builds. Network 5000001 ran the first build. Network 5000002 ran the final Rust and the first image, and its fsck reported `garbage: 4` (harmless `tmp_pack_*` files from 409 retries, which the image of record now removes). Both had 0 failures. Every container started in its hinted region: ENAM in atl06 or mia10, WEUR in ams13 or cdg15, and none was replaced.

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| BF-18 | Archived 7 inline packs into a promoted `DurablePack`, truncated their entries, killed both shards, and rehydrated from markers plus the log after the promoted range. That included a stored pack whose marker write was killed. The second shard, behind its `IngestCursor`, caught up across the truncated range and later rehydrated and served every newer push. `git fsck --strict` was clean on both shards. 9 of 9 clones (both shards, v2 and v0, plus every ref alone) matched the leader's refs and passed fsck, and the fetch was ok. 3 runs, 0 failures | `git fsck` clean, every ref clones, second shard serves the newer pushes; fail on any missing object | **Pass**, with the chain leader stubbed. It needs a rerun once the site script hosts `GitShard` and the network's Raft group serves `ChainLog` |

## Start-time region check and the 6 h renewal

- `region_check`, `may_start`, and `renewal` are pure and unit-tested in `crates/git/tests/shard.rs`. A container outside its site's container regions is replaced at most 3 times, then kept with an alarm. A missing `CLOUDFLARE_REGION` counts as out of region. An `Oc` site accepts OC and APAC. The alarm walk from a last request fires 24 times, never leaves a gap over 1 h (under the 6 h timeout), stops exactly 24 h after the last request, and never renews past that point, even when an alarm fires late. Of the planted bugs (listed in `m1-wp4.md`), 6 were in this logic and all 6 were caught. One of them, "warm until 24 h after the first request", was caught only after I added the late-alarm assertion.
- On Cloudflare, `ShardHost::up` read `CLOUDFLARE_REGION` from the container's `/health` at each start (the server is pid 1, so it sees the runtime's variables) and kept every container. Each `up` also called `setInactivityTimeout(6 h)`, which the runtime accepted. Each request armed the alarm about 1 h out (`/status`, `"alarm"`). No alarm was seen firing during the run. That needs at least an hour; the renewal it would run is the tested `renewal`, called from `on_alarm`. After the run, `/teardown` destroyed every container and deleted their alarms.

## Built here and not yet gated

- `shards_for` (the first shard by rendezvous among `count`, the other k − 1 from the hot pool), `warm_sites`, `validator`, `may_start` (`count + HOT_POOL`), `SHARDS = [2, 5, 25]`, as B0-14 kept them (`tests/m0/results/m0-wp6.md`: "the shard counts stay 2, 5, and 25"), `HOT_POOL = 7`, and `image_for` (`git` and `git_prev` over a `ContainerDay`'s 10%, 50%, and 100% steps in rendezvous order, with `Rollback` returning to `git_prev`). Unit tests check the hot-pool range, that raising k moves no network, the spread (25,000 networks over 25 shards, each shard between 850 and 1,150), that going from 25 to 26 shards moves about 1 in 26 networks and only onto the new shard, and the day steps. The bed uses fixed shard names, and its image map has `git` only.
- The LRU of repositories on a shard's disk is built (see "The LRU of bare repositories" below). The bed still runs one BF-18 network per shard, plus the LRU step's filler networks.
- `merge` and `predict` are other work packages.

## The LRU of bare repositories (added 2026-11-01, later)

The design gives each shard "an LRU of bare repositories with reftable" (`design/DESIGN.md`, Shape, "Git cache shards (derived)"). This section covers what was built.

- **Pure plan:** `shard::evict_plan(repos, disk_bytes, incoming, keep) -> Vec<NetworkId>`. It evicts repositories least recently used first until the bytes on disk, plus `incoming`, fit under `DISK_USE_PCT` (80 %) of the disk [guess: no measured figure for git's working headroom]. It never evicts `keep`, the network the current request is for, so a request whose own repository is over budget still runs and leaves only that repository. Ties on `used` break by network id, so two calls over the same listing agree. `DiskRepo { network, bytes, used }` is one listing row.
- **Container** (`containers/git/server/main.go`):
  - `GET /repos` returns the disk size (statfs on the data directory, or `GRAFT_DISK_BYTES`) and, for each repository, its bytes and `used`. `used` is the mtime of `<repo>/graft-used`, which every request on the network touches.
  - `POST /evict?net=` deletes a repository. It answers 409 while a request holds the repository.
  - Each network's lock is now an RWMutex. Upload-pack, fsck and promote hold it shared, so concurrent clones don't serialize, and `/evict` takes it exclusively with TryLock.
- **Host** (`crates/git/src/host.rs`, feature `worker`):
  - `ShardHost::repos()` lists the disk.
  - `ShardHost::make_room(network, incoming, disk_override) -> Room { disk, held, incoming, evicted, busy }` evicts per the plan. It deletes the evicted network's `IngestCursor` before deleting the repository, so the network's next request rehydrates instead of trusting a cursor that points at a disk the repository has left. If the container answers 409, it restores the cursor and lists the network under `busy`.
  - `rehydrate` calls `make_room` with the bytes its plan will ingest (`RehydrateReport.room`), and `catch_up` calls it before ingesting (`CatchUp.evicted`).
- **Unit tests:**
  - `crates/git/tests/shard.rs` has three: LRU order until under budget; ties and the incoming bytes; never evicting `keep`, including when `keep` alone is over budget.
  - `containers/git/server/lru_test.go` has three: `/repos` size and last use; `/evict` removes a repository and refuses one in use; concurrent readers of one network don't block each other.
  - All pass. Two bugs were planted in the server and both were caught: a missing `touch` in `netParam` (caught by the last-use test) and `Lock` instead of `TryLock` in `/evict` (caught by the in-use test).
- **On Cloudflare (BF-18 step 8, below):** `1-0`'s real container held the BF-18 network (2,757,229 bytes) and three empty filler networks 5000021 to 5000023 (27,544 bytes each, touched in that order after the BF-18 network was served). With a disk budget of 3,481,066 bytes, 80 % is 2,784,800 against 2,839,861 held. `make_room` for the BF-18 network evicted 5000021 and 5000022, the two least recently used fillers, because one eviction leaves 2,812,317 bytes, which is still over budget. It kept 5000020 and 5000023, and the BF-18 cursor stayed at 19. A `git clone --mirror` from `1-0` straight afterwards matched every ref, with no rehydrate. The step first failed on the bed itself: `/shard/<name>/<op>` dropped the query string, so `fill=3` never reached the object. This was fixed in `tests/m1/wp5/src/lib.rs`, and the LRU code needed no change.

## BF-18 rerun after the LRU (2026-11-01)

The commands were `cd tests/m1/wp5 && BF18_NET=5000020 worker-build --release && npx --prefix ../../m0 wrangler deploy`, then `WP5_URL=https://graft-m1-wp5.<workers-subdomain>.workers.dev WP5_TOKEN=… tests/m1/wp5/bf18.sh ~/.graft-wp5/run20`. The output is in `raw/bf18-net5000020.log`. The image is `graft-m1-wp5-gitshard-git:wrangler-mvy2psbi`, `sha256:b7bbc1f6644c428a7f27495fe676fa3d6772b02c73030ffcab3039b56e46b43e`: alpine:3.22 with git 2.56.0, 74.3 MB. It is the same digest the WP6 bed now runs. (Network 5000011 was skipped because its stub chain was already in use from an earlier attempt. Its first push was refused as `stale`, which is the `expect` check working.)

The steps match the table above, on network 5000020:

1. Pushes 1 to 8 placed 7 inline packs plus a 1,500,720-byte stored pack. `1-0`'s first call rehydrated from 1 marker and 7 inline packs; its `room` showed 0 evicted.
2. The archive of 1 to 8 committed `Promoted` (index 9) and `Archived` (index 10).
3. Pushes 11 to 14 included a 1,200,639-byte stored pack whose marker write was killed (index 13).
4. Both shards were killed.
5. `0-0` rehydrated from 2 markers, 3 stored and 3 inline packs (6 ingests, 2,706,430 bytes) in ENAM (ord02), with 0 replacements. `git fsck --strict` was ok, garbage 0.
6. Pushes 15 to 17 arrived while `1-0` was down.
7. `1-0` rehydrated from 2 markers, 3 stored and 6 inline packs (9 ingests) in WEUR (lhr10). After push 18, it caught up from its cursor: 1 entry, 1 pack. fsck was ok.
8. 9 of 9 clones matched and passed fsck: a mirror clone over v2 and over v0 from each shard, plus a single-branch clone of each of the 5 refs from `1-0`. The incremental fetch after push 19 was ok.
9. The LRU step (above) passed, as did the clone after it.

`failures: 0` and `failures after lru: 0`. Afterwards, `/shard/0-0/teardown` and `/shard/1-0/teardown` destroyed both containers and deleted their alarms.

| Row | Result | Verdict |
| --- | --- | --- |
| BF-18 (rerun with LRU) | As above. Every check passed, and the LRU evicted exactly the two least recently used fillers without touching the served network or its cursor | **Pass**, with the chain leader still stubbed |

## The sweeper's cron entry

`crates/git/src/sweep.rs`, `sweep_all(buckets, requests, now, budget, from, mine) -> Result<SweepAll, StoreError>`:

- It lists the networks under `pack/`, `committed/pack/` and `upload/` in every bucket, using a delimiter listing (`Bucket::segments`, which R2 implements, with a default for in-memory buckets).
- It keeps the networks `mine` accepts: each site passes "home site is me", so three sites don't repeat one another, though the sweep is idempotent either way.
- It runs `sweep` on each network in id order, starting at `from`, until `CronBudget { networks, actions }` runs out.
- It returns `SweepAll { swept, failed, next }`. The caller stores `next` and passes it back as `from`; `None` means the run reached the end and the next one starts over.
- `CronBudget::SITE_CRON` is 20 networks and 600 actions [guess, unmeasured]: 20 networks × 3 buckets × 3 listings is 180 subrequests, which leaves room under 1,000 for lookups and deletes.
- Three unit tests cover it: it finds every network and resumes after its budget; it stops inside a network when actions run out and continues there; it sweeps only the site's own networks and never deletes a committed pack. Two bugs were planted and both were caught: an off-by-one in the resume point, and `next` always `None`.

## BF-18 on the hosted install with a real `ChainLog` (2026-11-02)

BF-18: "Archive a range of inline packs, kill all shards, rehydrate. A second shard behind its `IngestCursor`. Pass: `git fsck` clean, every ref clones, second shard serves the newer pushes." Earlier runs stubbed the chain leader in the bed. This run uses no bed:

- **Pushes and clones.** Every push and clone is unmodified git 2.56.0 over smart HTTP through `graft-m1-edge` on the test install `graft-m1-site-a/b/c` (`test-faults` build).
- **Shards.** They are the site scripts' `GitShard` class: the home shard on site 0, and site 1's shard of the same network. `x-graft-shard: 1` with the bed token routes git requests to site 1's shard (test-faults only).
- **The log.** The `ChainLog` is the repository's Raft group. `GroupLog` in `workers/site/src/shard.rs` serves `first_index`, `entries` (with each entry's `promoted`), and `last_promoted` from the members' `/read` and `/entries`.
- **Driver.** `tests/m1/wp8/bf18.sh <workdir> <owner/repo> <id>`.

### What was built for it

- **`crates/git`.**
  - `RepoEvent::Promoted(Promoted)` (encode tag 2) and `RepoEvent::Archived(Archived)` (tag 3).
  - `RepoFold` now keeps `promoted` and `archived` ranges, and `first_index()` is the last `Archived.to + 1`.
  - `RepoFold::check` refuses five cases: a `Promoted` range that does not start right after the last one, or that names an unstored pack; an `Archived` range that does not start right after the last one; an `Archived` that reaches past the last promotion; and an archive key that does not name the chain, range and digest.
  - New test `promoted_and_archived_follow_each_other_and_archive_only_promoted_ranges` (tests/push.rs).
- **`crates/raft`.** `Member::record` commits events no command decided (`m1-wp2.md`, "Gate-row pass").
- **`workers/site`.**
  - The Repo family persists `promoted` and `archived` in its fold row, and `/read {"q":{"ranges":true}}` answers them.
  - After an `Archived` applies, every member HEADs the archive key in the install's buckets. With two or more copies it deletes its `log` rows below `to`, and keeps the row at `to` as the stored log's base. Restore accepts a truncated log and skips the replay cross-check when the log is truncated.
  - The shard route `/promote` packs the named packs' objects into one complete pack (`pack-objects` from the ingested packs).
  - A test-faults `/record` checks the events against the fold, then calls `Member::record`.
- **`workers/edge`.** The test-faults route `/_bed/archive?repo&from&to` runs the archive in the order the design gives (DESIGN, Shape, "Packs"):
  1. Read the range's entries from the leader.
  2. Promote on the home shard and validate the result.
  3. Place the promoted pack in two regions.
  4. Re-put the range's stored packs' markers.
  5. Have the leader record `Promoted`, then put the promoted pack's marker.
  6. Put the archive object (the range's entries, keyed by range and SHA-256 digest) create-only in two buckets.
  7. Have the leader record `Archived`.

### Run 1 (repository `wp6/bf18a`, 196621): failed at rehydrate

The archive of 1 to 8 worked: `Promoted` committed at index 9 and `Archived` at index 10, covering 5 inline packs and 1 stored pack. Every member truncated its log to start at 8.

After the shards were killed, the home shard's rehydrate failed:

```
ingest 5307aa09…: 422 git index-pack: exit status 128: fatal: did not receive expected object 6a36902c552c15b2e94e37e121d6c22a390c3c98
```

The run counted 14 failures. Every later push and clone hit the same ingest (`tests/m1/wp8/raw/r1.bf18.log`).

**Cause (verified).** The promotion packed only the range's inline packs, as the design says. Pushed packs are stored thin. The range's stored pack (index 7, the 1.5 MB blob push) had its delta bases in earlier inline packs, and the inline pack after it (index 8) has trees that name the stored pack's objects. So the promoted pack and the stored pack each needed the other. Rehydrate ingests each pack on its own with `index-pack --strict --fix-thin`, so whichever came first could not resolve the other's objects. The fix was in the bed, not in rehydrate: `bed_archive` now promotes the stored packs' objects too (it passes their checksums to `/promote`, since the home shard ingested each under its checksum). The promoted pack then holds every object the range names and is complete on its own. See "Design problems found" below.

### Run 2 (repository `wp6/bf18b`, 196623): pass

Run `bf18-1793621083`, 12:04 to 12:12Z (`tests/m1/wp8/raw/r2.bf18.log` and `r2.bf18.steps.jsonl`):

1. **Pushes 3 to 8:** 6 pushes, all through `git push`. Indexes 3 to 6 and 8 are inline packs. Index 7 is a 1,500,000-byte random blob, a stored pack with markers.
2. **Catch-up before the archive.** Shard 1 rehydrated and stopped at cursor 8. Shard 0 caught up to 8.
3. **Archive 1 to 8.** `Promoted` committed at index 9: 5 inline packs plus the 1 stored pack, 39 objects in 1,503,681 bytes, 0 marker failures, 1 marker re-put. `Archived` committed at index 10, key `archive/repo:196623/…0001-…0008-e02670ae…`, in buckets 0 and 1.
4. **After the archive.** Every member's stored log starts at index 8. Read through each member's `/log` after it had restored once more: indexes 8 to 17 on sites 0, 1 and 2.
5. **Pushes 11 to 13.** One of them, at index 13, is a 1,200,000-byte stored pack. Shard 0 caught up to 13.
6. **Both shards killed** (container destroyed and alarm deleted). Shard 0 reported `running: false`.
7. **Shard 0 rehydrated** in ENAM (iad15): 3 markers (the promoted pack and both stored packs), 3 stored packs, and 2 inline packs from the log after the last `Promoted` (indexes 11 and 12). That is 5 ingests and 4,206,309 bytes, with 0 replacements. `git fsck`: ok, garbage 0. Its refs matched the leader's.
8. **Pushes 14 and 15 while shard 1 was down,** behind its cursor of 8. Shard 1's cursor was below the archived range, so it rehydrated in WEUR (lhr01): 3 markers, 3 stored packs, and 4 inline packs (11, 12, 14, 15). That is 7 ingests, ending at cursor 15. After push 16 it caught up from its cursor, 1 entry and 1 pack, to 16. `git fsck`: ok.
9. **Clones.** There are 5 refs. Mirror clones over v2 and v0 from each shard matched the leader's refs and passed `git fsck --strict --no-dangling`: 4 of 4. A single-branch clone of each ref from shard 1 matched and passed `fsck --strict`: 5 of 5.
10. **Fetch.** After push 17, an incremental fetch from shard 1 into its v2 mirror returned the new `main`.

`failures: 0`.

| Row | Result | Verdict |
| --- | --- | --- |
| BF-18 (hosted, real `ChainLog`) | Run 2 above. The archived range's 8 entries were truncated on all three members. Both shards rehydrated from markers plus the log after the last `Promoted`, and fsck was clean on both. 9 of 9 clones matched and passed fsck, and the fetch after the newest push was ok. Run 1 failed for the cause above | **Pass** (run 2), on the bed's archive trigger (below) |

### What is still stubbed or short

- **The archive is triggered by a test route** (`/_bed/archive`), not by a leader policy. The design gives no trigger or threshold.
- **A key held in one bucket is not re-put.** A member that HEADs a committed key in fewer than two buckets only alarms and skips truncation. The design wants it to re-put the same bytes create-only.
- **`last_promoted`'s index is an upper bound,** the read's applied index. The fold does not keep the index of the entry that carried `Promoted`, and rehydrate reads only the range.
- **`consumed` is empty.** No `Compacted` exists yet.
- **`RepoFold::check` is stricter than the design.** The design allows "a range that names none needs no `Promoted`". The check requires every archived range to be promoted first, so a range with no inline pack would need an empty promotion. No run met that case.
- **Run counts.** One passing run and one failing run on the hosted install. The earlier stubbed runs ran 3 times.

### Design problems found

1. **A promotion of only the inline packs is not self-contained when a stored pack in the range is thin.** DESIGN, Shape, "Packs": "Before the leader archives a range, the network's home shard packs the objects of every inline pack the range names into one `DurablePack`, and the leader commits `Promoted`." With the next sentence's "rehydrates a network from every stored pack with a marker, … plus the packs, inline or stored, named in each member chain's entries after its last `Promoted` range's `to`", a range that holds a thin stored pack yields two packs that each need the other's objects. The same section notes "Pushed packs are stored thin" and "`Promoted` packs are complete". Measured: run 1 above, `index-pack: fatal: did not receive expected object 6a36902c…`. With the stored pack's objects also packed into the promotion, run 2 passed. Either the promotion should pack every object the range's packs hold (as the bed now does, which duplicates the stored packs' objects), or rehydrate must ingest the promoted pack and the range's stored packs together (one `index-pack` over their concatenation, or ingest order plus `--fix-thin` against the repository rather than per pack).
