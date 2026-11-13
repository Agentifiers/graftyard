# `rehydrate-batch`: batched ingest for a cold shard rehydrate (2026-11-13)

Branch `rehydrate-batch`, from main `20147db`. Follows `tests/am1/results/hot-shards-2.md`, where a cold 2.05 GB rehydrate took 20 to 22 min on `standard-2`, its tail 66 to 72 ms a pack against about 10 ms of `index-pack` locally, and named batched ingest as the next lever. The goal: a cold rehydrate of about 2 GB of log finishes well under 10 minutes.

## Summary

- **Batched ingest roughly halves a hosted cold rehydrate on `standard-4`.** Repository `m3org/hot-rb` (`repo:65537`) on `graft-m3f`, with 2.21 GB of log above `Promoted`: the rehydrate loads 341 stored packs, then a 1.66 GB tail of 8,145 inline packs in 4,484 entries. All three sites' shards were killed and rehydrated at once.

  | Shard image | Whole rehydrate, sites a / b / c | Tail | Calls |
  | --- | --- | --- | --- |
  | Baseline, `15f6ed1b` (hot-shards-2), `standard-4` | 390 / 443 / 454 s | 378 to 436 s, 46.4 to 53.6 ms a pack | one per pack: 8,145 inline + 341 stored |
  | `rehydrate-batch`, `6c2a6461`, run 1 | 197 / 276 / 264 s | 151 to 228 s, 18.5 to 28.0 ms a pack | 1,289 (incl. 341 stored) |
  | `rehydrate-batch`, run 2 | 168 / 246 / 225 s | 155 to 230 s, 19.1 to 28.2 ms a pack | 1,289 |

  Every batched rehydrate finished in under 4.6 min, against the 10 min goal. The baseline's numbers are for the same tail. A first baseline on the smaller 1.56 GB tail (5,808 packs) took 284 to 354 s.
- **`standard-4` alone already brought the old code under 10 min.** hot-shards-2 measured 20 to 22 min for 2.05 GB inline on `standard-2`, at 66 to 72 ms a pack. The baseline here, before any code change, took 46 to 54 ms a pack. The tails differ (about 203 KB a pack here, 130 KB there; see "Contradictions"), and the 2.05 GB shape was not rerun on `standard-4`.
- **`git fsck --strict` was clean on all three shards after the batched run**, each with 501 refs and the same ref digest (`4181328bbba07a6f`), 35,171 objects in one pack after the catch-up's repack.
- **The shard held at most 5,964,824 bytes of log pages**, the same two-page bound as before (6 MB). A batch call's body is a copy of part of the page being applied, outside the wasm heap.
- **Resume after a cut-off rehydrate, hosted: it resumes, finishes, and fsck is clean with identical refs.** A test-faults `ctx.abort()` (`/_bed/shard_abort`) cut site a's shard object 70 s into a cold rehydrate, mid-tail, with its container and disk kept. Once the object came back in a new isolate, its alarm resumed the run on the same disk: `"resumed": true`, the marked phase skipped, tail 83 s for the 5,195 packs left (898 MB in 780 calls), held_max 5,949,850. `git fsck --strict` was then clean on all three shards with 501 refs and the same digest `4181328bbba07a6f` as before the cut. That first cut exposed two shard bugs, fixed in `119fcaa` (see "Resume, hosted"). **With the fix, run hosted (08:11Z, sites `254f392c` / `5d47101f` / `c296807a`, image `696907a2`):** site a's object, aborted 70 s into a cold rehydrate, resumed by itself and finished 90 s after the cut (`"resumed": true`, tail 6,078 packs, 934 MB in 891 calls, held_max 5,964,824). Site b, killed again at 70 s, rehydrated a new disk in 284.8 s, reading every page from its own site's member (1,290 calls), against 698 s before the fix. `git fsck --strict` was clean on all three shards: 501 refs each, digest `4181328bbba07a6f`, the same as before both cuts.

## Change

| Piece | What it does |
| --- | --- |
| `POST /ingest-batch?net=N` (`containers/git/server/main.go`) | Body: per pack a `<sha256> <bytes>` line, then its bytes. Each pack's checksum is checked as it spools. Packs already ingested answer `done`; a pack `/validate` kept aside is adopted as `/ingest` adopts it. The rest are concatenated into one pack (`concatPacks`: entries copied as they are, so `OFS_DELTA` offsets stay right) and indexed with one `index-pack --strict --fix-thin`. Each pack's objects are recorded under its own SHA-256 by their offsets in the joined pack, so `/promote` packs exactly what `/ingest` would have recorded; bases `--fix-thin` appended are no pack's. When the joined pack fails, the run splits in halves down to one pack, which is ingested as `/ingest` does: `later` for a missing base, 422 for anything else. Two pushes that carry the same object, which `--strict` refuses in one pack ("appears twice"), end up in different halves. The multi-pack-index is rewritten after each batch on the `midxEvery` count |
| `ShardIo::ingest` takes a run of packs (`crates/git/src/catchup.rs`) | `ingest_all` sends each run of inline packs between stored ones in calls of up to `INGEST_BATCH_BYTES` (4 MB, one local `/entries` page); a pack answered `later` is sent again after the rest, as before. A stored pack still streams alone from its bucket. `Rehydrated.calls` counts the calls |
| `Io::ingest` (`crates/git/src/host.rs`) | Writes the batch body straight into one JS `Uint8Array`, outside the wasm heap. A container on an image without the route (404, `git_prev` during a container day) gets one `/ingest` a pack |

Memory: the shard object still holds at most two pages (8 MB bound; 5.96 MB measured); a call's body is one copy of up to 4 MB of the page being applied, outside the wasm heap. The container spools the batch to disk.

## Local measurement

`tests/am1/hotshards/bench/batchserve.py`: the container server built from this branch, on one CPU (`taskset -c 0`), git 2.56.0, packs from `tests/am1/hotshards/bench/gen.mjs`, each pack thin-free and on its own branch chain. `git fsck --strict` clean after every run. Log: `tests/am1/raw/rehydrate/bench/batchserve.jsonl`.

| Packs | One `/ingest` a pack | `/ingest-batch`, 1 MiB | `/ingest-batch`, 4 MB |
| --- | --- | --- | --- |
| 9,000 of 20 KB | 94.3 s (10.5 ms a pack) | 5.9 s, 177 calls (0.65 ms) | 4.9 s, 44 calls (0.55 ms) |
| 2,000 of 500 KB | 22.0 s (11.0 ms a pack) | 20.2 s, 1,000 calls (10.1 ms) | 16.4 s, 250 calls (8.2 ms) |

Small packs gain 19x: `index-pack`'s per-run cost, not the bytes, dominated. 500 KB packs are bound by inflating and hashing their bytes, so the local gain is 1.3x. Hosted, the gain on those packs comes from the round trips, which local runs do not have.

## Hosted run on `graft-m3f`

Install `graft-m3f` (`tests/am1/results/install-m3f.md` on branch `install-m3ef`), shared with merge-p99 by its site-day log; every deploy here was announced to that thread first. Sites a (enam), b (weur), c (apac), shards `SHARD_INSTANCE=standard-4`. Raw files are in `tests/am1/raw/rehydrate/` (gitignored); `timeline.txt` lists every phase.

**Seed.** Repository `m3org/hot-rb` (`repo:65537`, `Tri`, home site a), created at 04:14Z. Pushes through `graft-m3f-edge` by m3-admin, as hot-shards seeded hot-a (`tests/am1/hotshards/load.mjs` through `tests/am1/rehydrate/run.sh`), with the archive held by `archiveCap` every 15 s (`tests/am1/hotshards/hold.mjs`):

| Phase (Z) | Pushes ok | Bytes |
| --- | --- | --- |
| 04:16 to 04:31, 500 KB packs at 1.9/s | 1,714 | 0.858 GB |
| 04:32 to 04:39, journey 9's sizes (10 to 70 KB, 5% 1 MiB) | 3,088 | 0.290 GB |
| 04:39 to 04:52, 500 KB packs | 1,491 | 0.746 GB |
| 05:09 to 05:19, journey 9's sizes | 2,447 | 0.248 GB |

Promote-ahead still ran a few times under the cap (promotedTo 30 to 306), and the log held 2.21 GB above `Promoted` from 05:19Z. A rehydrate then loads 341 stored packs (the promotions and the pushes the edge staged), then entries 307 to 4,792: 8,145 inline packs, 1.656 GB.

**Measurement.** `tests/am1/rehydrate/cold.mjs`: `/_bed/kill` of repo 65537's first shard on all three sites at once, then a catch-up on each, which rehydrates on a new disk in the shard object. Times are the shard's own laps from its Workers Logs line "shard rehydrate 65537" (`tests/am1/hotshards/logs.py`), from the container's start to the end of the catch-up after the refs.

| Run (Z) | Image | Tail packs | a | b | c |
| --- | --- | --- | --- | --- | --- |
| 04:58 to 05:05 | `15f6ed1b` | 5,808 (1.56 GB) | 284 s (tail 277 s, 47.6 ms a pack) | 353 s (339 s, 58.4 ms) | 335 s (321 s, 55.3 ms) |
| 05:20 to 05:28 | `15f6ed1b` | 8,145 (1.66 GB) | 390 s (378 s, 46.4 ms) | 443 s (428 s, 52.6 ms) | 454 s (436 s, 53.6 ms) |
| 05:39 to 05:44 | `6c2a6461` | 8,145 | 197 s (151 s, 18.5 ms), 1,289 calls | 276 s (228 s, 28.0 ms) | 264 s (211 s, 25.9 ms) |
| 05:45 to 05:50 | `6c2a6461` | 8,145 | 168 s (155 s, 19.1 ms) | 246 s (230 s, 28.2 ms) | 225 s (207 s, 25.4 ms) |
| 06:03 to 06:06 (site a only) | `6c2a6461` | 8,145 | 167 s (152 s) | | |

The marked phase (341 stored packs, one bucket read each) took 6 to 17 s, except 44 to 52 s in the first batched run, the first start of the new image on each site. Container CPU during the baseline tail read 3 to 5% of 4 vCPU (`/status` `load`), so the old tail waited on round trips, not on `index-pack`.

The deploy for the batched runs: `rb-deploy` `f810040`, which is `rehydrate-batch` merged with `origin/merge-p99` `2f5f25d` (the other thread's site and edge changes, so neither rolled the other back). Sites `825e04ba` / `056f7fcf` / `1b922273` at 05:38Z, then `0176381f` / `1edbdf20` / `d94dcac3` after the resume tries; edge `0bac3f72`. Shard image `graft-m1-site-gitshard-git@sha256:6c2a6461f9df4a0314a4e251f08a45e625560656bc7249e37192adb65ff2c932`, built from `rehydrate-batch`'s `containers/git` (`TAG=rb bash tests/am1/hotshards/image.sh`).

**fsck.** `tests/am1/rehydrate/fsck.mjs` after the first batched run: all three `ok`, 501 refs each, the same refs digest, 35,171 objects (`fsck-batch1.log`).

## Tests and plants

- `containers/git/server/batch_test.go`, `TestIngestBatchMatchesOneIngestPerPack`: two packs carrying the same blob in one call, a pack thin against an earlier pack of the same call, a pack whose base no pack of the call brings (`later`, no object left behind), the same pack sent before its base in a later call, a pack already ingested, and a corrupt pack (422). Each pack's recorded objects equal what `rev-list --objects` says it carries; `fsck --strict` clean.
- `crates/git/tests/catchup.rs`, `a_rehydrate_ingests_runs_of_inline_packs_a_call_at_a_time`: 300 tail packs in at most one call a page plus one, and a pack mid-call whose base comes later is sent again alone. The model container answers each pack of a call in order, as the server does. `a_rehydrate_resumed_after_a_new_promotion_loads_it_and_skips_nothing`'s cut points moved into the tail (40 and 60 were in it before; 90 now falls after the end, since a cut counts calls).
- Resume after a cut-off rehydrate: `a_cut_off_rehydrate_resumes_and_never_starts_again_from_nothing` (cuts at 7 to 180 calls), `a_rehydrate_resumed_after_a_new_promotion_loads_it_and_skips_nothing`, and `a_rehydrate_resumed_from_an_older_place_loads_the_marked_phase_again` pass unchanged in what they assert, with every cut now landing inside a batch call. `a_rehydrate_resumed_from_an_older_place_loads_the_marked_phase_again` cut at 40 calls, which batching moved past its tail, so plant 35 slipped through; it now cuts at 14 calls, two into the tail, and asserts the cut falls there.

Planted bugs (`tests/am1/hotshards/plant.py`, log `tests/am1/raw/rehydrate/plants.log`): all 44 caught, none missed, all compiled (one run of all 44; plant 35 rerun after its test was fixed). The ones new on this branch:

| # | Mutant | Caught by |
| --- | --- | --- |
| 38 | each inline pack is still one ingest call | `a_rehydrate_ingests_runs_of_inline_packs_a_call_at_a_time` |
| 39 | a pack answered `later` mid-batch is retried as the batch's first pack | same |
| 40 | a batch never splits: two packs carrying one object fail the call | `TestIngestBatchMatchesOneIngestPerPack` |
| 41 | a batch records every object under its first pack | same |
| 42 | a batch records the thin bases `--fix-thin` appended as the last pack's | same |
| 43 | a pack whose base is missing fails the batch instead of answering `later` | same |

One more mutant, which ingested an already-ingested pack again, behaved the same as the code (re-indexing a held pack is idempotent, and the caller asks `/have` first), so it was removed.

## Hosted drivers

`tests/am1/rehydrate/run.sh` runs the hot-shards drivers against `graft-m3f` (`tests/am1/lf13/env-m3f.sh`). `tests/am1/hosted/lib.mjs` gained `voterNames`: on `graft-m3f`, site b's voter of `repo:65537` had been replaced at generation 3 (`repo:65537#3#f1`), and `/bench_seed` by site alone reached the stale generation-1 object at applied index 1, so the seed's level and the archive hold missed that member. `hold.mjs`, `load.mjs` and `monitor.mjs` now call each site's current voter. `tests/am1/rehydrate/cold.mjs` kills repo's first shard on each site and times the rehydrate; `tests/am1/rehydrate/fsck.mjs` runs `/_bed/fsck` on each and digests the refs.

## Cost

`INSTALL=graft-m3f python3 tests/fanout/m2bcost.py 2026-11-13T04:10:00Z 2026-11-13T06:08:00Z`: **$7.48** for the whole install, containers $2.06, which includes merge-p99's 500-agent smokes in the same window; this branch's share is below that. Under the $20 cap. Local work used the orb only.

## Design changes

- `design/DESIGN.md`, Shape, `rehydrate`: inline packs go to the container a call per run of up to 4 MB, one `index-pack` of their concatenation, split in halves when two packs share an object [measured, this file].
- `decisions/DECISIONS.md`: "A shard ingests a run of inline packs in one call, as one `index-pack` split on failure".
- `decisions/OPEN-QUESTIONS.md`: G7 updated.

## Contradictions and open items

- **Resume, hosted (06:37 to 07:47Z, sites `95b96511` / `75eab7f8` / `0fabe172`, image `6c2a6461`).** `tests/am1/rehydrate/cut.mjs` killed repo 65537's shards on sites a and b. At 70 s it aborted site a's object (`/_bed/shard_abort`) and killed site b's container again (a new disk). Two bugs showed:
  - **The aborted object stayed stuck for 46 min.** The object came back in the same isolate, and `RUNS` was keyed by the object id, so the new instance found the aborted run still marked running. Every catch-up and alarm then waited on a future that never ended (`/status` `running: true`, container idle, no ingest). merge-p99's 07:24Z site day put the object in a new isolate, and the run resumed and finished (above). Fix: `RUNS`, `CURSORS` and `REPORTS` are keyed by the object instance (`workers/site/src/shard.rs`, `run_key`).
  - **Site b rehydrated in 698 s, against 246 to 276 s before.** Its shard read its "local" member by the record's bootstrap name. Site b's voter had been replaced (`repo:65537#3#f1`, generation 3), and the retired first-generation object answered no entries, so every page came from the leader at 600 KB, about one entry a page (3,633 calls for 8,145 packs, against 1,289). Fix: the local read learns the committed config's voters first (`graftyard_lookup::learn_voters`). The earlier batched runs were not affected: their isolates had already learned the voters from a call through the lookup.
  - A first hosted try at a cut (two site-only redeploys, 06:01Z and 06:04Z) did not cut. The busy object stayed on its version until its run ended (`"resumed": false`, 167 s).
- **The 2.05 GB shape was not rerun: the $20 cap ran out first.** The plan was to top up repo 65537's 8,145-pack tail with about 7,700 packs of 30 to 72 KB (`SEED_PACK=30000-72000 SEED_N=7700`), reaching about 15,800 inline packs and 2.05 GB, then time a cold rehydrate on all three sites. The top-up ran at 08:18Z at 8/s, but pushes took 10 to 12 s each (p50) and landed at about 2.8/s, and the install's cost reached $18.49 of $20 at 08:20Z. It stopped at 679 pushes (08:23Z), the hold was released, and 65537's shards were torn down without a timed run.
- **Earlier note on the 2.05 GB shape.** hot-shards-2's tail was 15,820 packs of about 130 KB on `standard-2`; this one is 8,145 packs of about 203 KB on `standard-4`. Per pack, the old code took 66 to 72 ms there and 46 to 54 ms here, so `standard-4` alone explains part of the gain. Scaled per pack, 15,820 packs at 19 to 28 ms is 5 to 7.4 min of tail with batching, against 12 to 14 min without [inference].
- **Sites b and c stay about 1.5x slower than a with batching.** Their tails took 207 to 230 s against a's 151 to 155 s; each call still waits on one container round trip and one `index-pack` of up to 4 MB. Not explained further.
- **`amp orb service` restarts a command when it exits.** The first site day ran twice (members reset at about 05:34, 05:36 and 05:37Z), and two cold runs started a second kill after they ended (stopped within a minute). Site days now run as plain background shells (`tests/am1/rehydrate/siteday.sh`, `resume.sh`).
