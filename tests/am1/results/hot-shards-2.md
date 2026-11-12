# `hot-shards-2`: closing G7 (2026-11-12)

Branch `hot-shards-2`, from `origin/b030-split` `f0f48b4` (which merges `hot-shards` `fcc604f` and main `fd6e711`). Follows `tests/am1/results/hot-shards.md`, whose "Contradictions and open items" left three gaps: promote stalls while the home shard rehydrates, rehydrate takes 30 to 43 min for 2.1 to 2.8 GB, and 11% of pushes paid a 15 s first hit after a kill. A31 was left needing a run designed for it.

## Summary

- **Pushes stayed under the warm p99 through a home-shard kill.** Run j9f: 2,393 s at journey 9's rate (7,651 pushes, 3.20/s) on a group with 2.73 GB of inline packs above `Promoted`, through two leader transfers and a kill of the home shard. After the kill, p99 was 7.66 s; from 2 min after it, 7.53 s with no push over 8.8 s. The warm window before the kill had p99 10.45 s. j9e had 11% of pushes over 15 s after its kill and p99 19.2 s; j9f had 7 of 5,327.
- **Promote and archive kept up through the kill.** The archive promoted on sites b and c while site a's shard rehydrated: promotedTo went from 5,379 at the kill to 6,840 by 22:04Z and 17,678 by 22:31Z, and the log above `Promoted` fell from 2.41 GB to 0.18 GB.
- **`git fsck` was clean on all three shards after catch-up**, with 504 refs each and the same refs on every shard.
- **Rehydrate is about 1.7x faster, not minutes.** A cold 2.05 GB rehydrate took 1,212 to 1,335 s (20 to 22 min), against 30 to 43 min on `hot-shards`. Per-pack round trips now dominate; batched ingest is the next lever.
- **The hosted run found a resume bug, fixed in `d88f8a9` and `41b51ba`.** A rehydrate cut off by the kill resumed with its tail past a promotion recorded meanwhile, and every call then failed on missing bases. After the fix, site a's resumed rehydrate finished clean.
- **A31 is not answered.** k was 8 throughout, carried over from earlier runs, so no step could show. The run that would answer it is designed below.

## Changes

| Gap | Change | Commit |
| --- | --- | --- |
| 1. Promote stalls while the home shard rehydrates | The archive alarm's promotion tries its own site's shard, then each other Full site's of the record (never a Witness), moving on for any answer but the packs' own 422. Each site binds every site's `GitShard` class, its own first, so the alarm can reach another site's shard | `8ba9a55` |
| 2. Rehydrate takes tens of minutes | The container writes a multi-pack-index every 50 ingests, so per-pack `index-pack` stops slowing down as packs pile up. A shard's read of its own site's member asks for `/entries` pages up to 4 MB, not 600 KB, so a hot tail of 500 KB packs is no longer one member read per pack | `c7a1b03`, `af73840` |
| 3. The 15 s first hit after a kill | Every shard try but the last carries `next=1`; a shard with a rehydrate under way answers it 503 `warming` at once, whatever edge isolate asked. The archive's promotion does the same | `3b6b809` |
| Resume after a promotion | A cut-off rehydrate records the promotions it loaded, and a resumed run reruns the marked phase when a later `Promoted` was recorded or its place came from an older release (found by the hosted run) | `d88f8a9`, `41b51ba` |
| A31 | The shard's `/status` reports the container's `NetworkLoad` (derived work left out) and the k it last read, so the monitor records each minute's load beside the committed k | `4612316` |

### 2. Rehydrate time: what was measured

Local, one CPU (`taskset -c 0`, a standard-2's share), git 2.56.0 as in the image. Scripts in `tests/am1/hotshards/bench/`, logs in `tests/am1/raw/hotshards2/`.

| Option | Measurement | Verdict |
| --- | --- | --- |
| Per-pack `index-pack --strict --fix-thin` as now | 9,000 packs of 20 KB: 720.7 s. Per 1,000 packs it went from 19.8 s (first 1,000) to 124.4 s (last 1,000) | The cause: each ingest looks objects up in every pack on disk |
| Same, with a multi-pack-index rewritten every 50 packs | 9,000 packs: **95.7 s** (5.3 s to 17.1 s per 1,000). 2,000 packs of 500 KB: 22.3 s, against 50.2 s without, the midx writes 0.6 s of it | **Chosen** (`c7a1b03`): 7.5x on the 9,000-pack shape, no change to the wire, the log, or R2 |
| An incremental midx chain every 50 | 9,000 packs: grew to 81 s per 1,000 by 7,000 | Rejected |
| `index-pack --threads=4` (a standard-4) | 600 packs of 500 KB: 6.7 s against 6.6 s on one thread | Rejected: thread count does not help one small pack at a time |
| `git repack --geometric=2` during ingest | 48 s per 500 packs of 500 KB | Rejected |
| Concatenate a batch of packs into one `index-pack` | 9,000 small packs in batches of 512: 2.8 s; of 16: 17.8 s. But `--strict` refuses an object that appears twice in one pack, and two pushes can carry the same object, so it needs a dedupe pass and a new ingest route keyed by many checksums | Not taken this round: the midx gives most of it with no format change. Next lever if minutes are still too long |
| Keep each stored pack's `.idx` in R2 | Copying a pack plus its prebuilt `.idx` and writing a midx: 1.36 s for 600 packs of 500 KB, against 8.4 s | Not taken: the tail that dominates is inline packs in the log, which have no R2 object to keep an `.idx` beside; stored packs were 10 to 20% of rehydrate time on hot-a |
| Rehydrate from the newest promoted pack plus the tail | Already the shape on main: stored packs below the last `Promoted` load first, then only the tail above it (`422bc02`, `19eca55`, hot-shards' `72ee373`) | No change |

The hosted rehydrates in `hot-shards.md` also ran one member read per tail entry: a member's `/entries` page stops past 600 KB, and each of hot-a's seed entries is one ~500 KB pack, so the tail read 2.6 GB as about 5,000 local reads of 60 to 120 ms each. A local read now asks for up to 4 MB (`bytes=`), which only a read of the shard's own site's member gets; a remote read keeps 600 KB so it answers within the lookup's CALL across regions. The shard holds two pages, so its memory bound becomes 8 MB.

## Tests and plants

- `crates/git/tests/catchup.rs`: `a_rehydrate_resumed_after_a_new_promotion_loads_it_and_skips_nothing` and `a_rehydrate_resumed_from_an_older_place_loads_the_marked_phase_again`.
- `crates/git/tests/shard.rs`: `a_promotion_tries_its_own_site_then_the_other_full_sites` (own site first even when the record lists it last or it is not the home, then the other Full sites in record order, never a Witness; 503, 409, 500 and a failed call move on, the packs' 422 does not), and `a_shard_answers_warming_at_once_only_to_a_caller_with_somewhere_else_to_go`.
- `workers/edge/src/lib.rs`: `every_shard_try_but_the_last_says_another_follows`.
- `workers/site/src/host.rs`: `only_a_local_entries_read_gets_a_larger_page`.
- `containers/git/server/repack_test.go`: `TestIngestWritesAMultiPackIndexEveryMidxEveryPacks` (107 real ingests: the midx covers exactly 100 packs, verifies, and the catch-up's repack leaves the repository whole with `fsck --strict` clean). Fails with the midx write disabled (checked by hand).

Planted bugs (`tests/am1/hotshards/plant.py`, log `tests/am1/raw/hotshards2/plants.log`): all 38 caught, none missed, all compiled. The ones new on this branch:

| # | Mutant | Caught by |
| --- | --- | --- |
| 23 | a promotion falls back to a Witness site | `a_promotion_tries_its_own_site_then_the_other_full_sites` |
| 24 | a promotion tries the home site first, not the leader's own | same |
| 25 | a promotion moves on after the packs' own 422 | same |
| 26 | a promotion gives up on its own shard's 503 `warming` | same |
| 27 | a rehydrating shard answers `warming` at once even to a caller's last try | `a_shard_answers_warming_at_once_only_to_a_caller_with_somewhere_else_to_go` |
| 28 | a shard answers `warming` at once to any caller with somewhere else to go, rehydrating or not | same |
| 29 | a cold pool shard waits on a forwarded push | same |
| 30 | the edge's last shard try also says another follows | `every_shard_try_but_the_last_says_another_follows` |
| 31 | the edge's home try waits as long as the others | same |
| 32 | a remote `/entries` read gets the large page too | `only_a_local_entries_read_gets_a_larger_page` |
| 33 | a local `/entries` read is not capped | same |
| 34 | a resumed rehydrate never loads a promotion recorded after its marked phase | `a_rehydrate_resumed_after_a_new_promotion_loads_it_and_skips_nothing` |
| 35 | a place an older release saved is trusted to have loaded up to its tail cursor | `a_rehydrate_resumed_from_an_older_place_loads_the_marked_phase_again` |
| 36 | ingest never writes a multi-pack-index | `TestIngestWritesAMultiPackIndexEveryMidxEveryPacks` |
| 37 | the multi-pack-index is written once and never again | same |

One more mutant, which skipped the marked phase's rerun when the place recorded no promotions, behaved the same as the code it replaced, so the plants could never catch it. It was removed.

## Hosted run on `graft-m3c`

Deploy: `hs2-deploy` `799c69f` (main `d7ee927` + `hot-shards-2` + merge-budget-3 + merge-budget-4), test-faults, image `sha256:15f6ed1b1d4b2b08c8adccbce713e89762677c3656dd7ee9a99bdd4bcab025c5`. Sites `c23c22b5`/`33eb501b`/`3868f3e5`; edge `ebd77ab4` (`--ops graft-m3c-ops`, FORMATS with 32.18). The cross-script `GitShard` bindings deployed without trouble. Two site-only redeploys kept the shards for the resume fix: `5eb916e`, then `4614375` (sites `bd507f40`/`f13e120e`/`80a331a6`), pushed as `origin/hs2-deploy`. Raw files are in `tests/am1/raw/hotshards/` (gitignored): `j9f-pushes.jsonl`, `j9f.log`, `faultsf.log`, `monitor.jsonl`, `fsck-j9f.json`, `timeline.txt`. The summary is `tests/am1/raw/hotshards2/j9f-summary.txt`.

### Cold rehydrate after the site day

All three sites, on the group m3org/hot-a (repo:65551): 2.05 GB inline (15,820 inline packs, 1,188 stored, 10,698 entries), finished at 21:20:13 to 21:22:16Z.

| | `hot-shards` | `hot-shards-2` |
| --- | --- | --- |
| Whole rehydrate | 30 to 43 min for 2.1 to 2.2 GB | 1,212 to 1,335 s (20 to 22 min) |
| Tail phase | 0.8 to 1.2 MB/s | 1,050 to 1,147 s, about 1.9 MB/s, 66 to 72 ms per pack |
| Most catch-up bytes held | | 5,970,193 (two 4 MB pages max) |
| Container memory | | about 3.3 GiB, tracking disk at about 3.0 GiB |

### j9f: pushes through two transfers and a home-shard kill

21:24:41 to 22:04:45Z, 2,393 s, 7,651 pushes at 3.20/s, 7,621 ok. Faults: transfer 1→2 at 21:30:42Z (term 57), kill of site a's shard 0 at 21:36:43Z, transfer 0→1 at 21:50:41Z (term 60). The log held 2.731 GB above `Promoted` when loading began.

| Window | Pushes | p50 | p99 | Over 15 s |
| --- | --- | --- | --- | --- |
| Whole run | 7,651 | 4.3 s | 8.8 s (max 57.3 s) | |
| First 3 min | 284 | | 55.8 s | 22 |
| Warm, 21:27:30 to 21:36:30 | 1,989 | 3.7 s | 10.45 s | |
| After the kill, to 22:05 | 5,327 (5,318 ok) | 4.6 s | 7.66 s (max 29.1 s) | 7 |
| From 2 min after the kill | 5,236 (all ok) | | 7.53 s (max 8.8 s) | 0 |

The first 3 minutes were slow because k = 8 was committed by earlier runs and site a's pool shards were cold. The 30 failures:

- 7 `http 503` at 21:32:09Z were self-inflicted: my `/_bed/log?n=20000` read reset site b's leader member with `exceededMemory`.
- 23 `ng unpacker error` at 21:36:38 to 21:36:44Z were pushes in flight at the kill. The edge maps any validate failure to `unpacker error`.

### Promote and archive through the kill

| Time (Z) | promotedTo | archivedTo | Above `Promoted` | Log |
| --- | --- | --- | --- | --- |
| 21:24 | 4,961 | 4,961 | 2.651 GB | 2.654 GB |
| 21:36 (kill) | 5,379 | 5,371 | 2.413 GB | 2.421 GB |
| 21:44 | 5,621 | 5,617 | 2.265 GB | 2.269 GB |
| 21:50 | 5,876 | 5,835 | 2.115 GB | 2.148 GB |
| 22:04 | 6,840 | 5,995 | 1.461 GB | 2.149 GB |
| 22:31 | 17,678 | | 0.178 GB | |

Promote calls after the kill: site a 50 (p50 5.2 s, max 60 s, while its shard rehydrated), site b 51 (p50 11 ms), site c 50 (p50 1.4 s). archivedTo stopped near 6,006 while the log stayed at 2.148 GB, under `LOG_MAX_BYTES`, which is likely by design [inference].

### The resume bug, and its fix

While site a's killed shard rehydrated, the archive kept promoting on sites b and c. The cut-off rehydrate resumed with its tail cursor past the new `Promoted`, so it never loaded that promotion's pack, and every call then failed `N packs of entries 6171..=6173 have bases no pack provides`. Site c's shards, reset by the redeploy, showed the same. `Rehydrating` now records the promotions it loaded (`loaded`, a serde default), and a resumed run reruns the marked phase when the last `Promoted` tail start is past what it loaded, or when it has no record (a place saved by an older release). After the fix, site a resumed (`resumed: true`) and finished at 22:40:43Z: marked phase 374 s (1,966 stored packs), tail 170 s (2,875 inline packs, 115 MB), cursor 19,347.

### fsck after catch-up

All three shards `ok`, 504 refs each, identical refs (`fsck-j9f.json`).

### A31

k was 8 (FANOUT_MAX) throughout, carried over from earlier runs, so this run could not show a step. `/load`'s `cpu_p99` read 0.99 on sites a and b at 21:20 to 21:23Z, the end of the cold rehydrates; whether derived work is fully left out under hosted load is unverified. The run that would answer it: force k = 1 with test-faults `/bench_seed {"fanout":1}` on the leader, kill the shards so they rehydrate and catch up with no pushes, and record `/status` `load` and `k` each minute past FANOUT_UP_FOR (3 min). If derived work is left out, k stays 1. Not run.

## Cost

`tests/am1/hotshards/usage.py` for graft-m3c from 20:38 to 22:45Z: **$2.88**. Containers are $2.38 (66,300 standard-2 instance-seconds from the metrics rows); Durable Objects and Workers are $0.50 (326,465 DO requests, 88,040 DO active seconds, 8,533 Workers requests). Local work used the orb only.

## Design changes

- `design/DESIGN.md`, Shape, `rehydrate`: a multi-pack-index every 50 ingests keeps per-pack ingest flat. `IngestCursor`: a rehydrating shard answers `warming` at once to a caller with another shard next; pushes, fetches and the archive's promotion try the home or own site, then the other Full sites.
- `design/DESIGN.md`, `rehydrate`: its saved place records the promotions it loaded, so a resumed run loads later ones.
- `decisions/OPEN-QUESTIONS.md`: G7 and A31 updated.
- `crates/raft/src` is unchanged.

## Contradictions and open items

- **Rehydrate is not minutes.** 2.05 GB takes 20 to 22 min, 1.7x faster than before, against a target of minutes. The tail costs 66 to 72 ms per pack in round trips and per-pack `index-pack`. The next lever is batched ingest: concatenating packs measured 2.8 to 17.8 s locally for 9,000 packs, but it needs a dedupe pass for `--strict` and a new ingest route.
- **Pushes in flight at a kill answer `unpacker error`.** 23 did. The edge maps any validate failure to that; a retryable status would be better.
- **A31 is not answered.** k = 8 carried over from earlier runs. The run is designed above but not run.
- **7 self-inflicted 503s.** A `/_bed/log?n=20000` read reset site b's leader member with `exceededMemory`. That debug route reads too much in one go.
- **Subrequest depth did not recur.** merge-budget-3 saw a cold catch-up of repo:65551 fail "Subrequest depth limit exceeded" at 20:07Z, one `/entries` hop deeper per call. Workers Logs for GitShard and Member on all three sites from 20:40 to 22:44Z hold no message containing "depth" (a control query for "rehydrat" does match), so j9f's rehydrates and catch-ups did not hit it. The 4 MB local pages mean fewer `/entries` calls per catch-up, but whether a call chain can still deepen is not proven [inference].
- **The first 3 minutes of j9f** had p99 55.8 s, from cold pool shards under the carried-over k = 8, not from the kill.
