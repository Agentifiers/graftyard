# `hot-shards`: git cache shards on a hot group with a multi-GB log (2026-11-12)

Branch `hot-shards`, from `origin/main` (`9a6d955`, later merged up to `eb2355b`). AM1-WP1, "Follow-up: git cache shards on a hot group with a multi-GB log" (`build/BUILD-PLAN.md`); `decisions/OPEN-QUESTIONS.md`, G7 and A31; `tests/am1/results/b0-30.md` on branch `b0-30`, "Blocked on". Hosted on the test install `graft-m3c` (`tests/am1/results/install-m3c.md`), with `origin/b030-split`'s promote-ahead merged into the deploy tree only. Nothing ran on `graft-m1-*` or `graft-m2b-*`.

## Summary

Rehydrate and catch-up are now bounded-memory, resumable, and incremental. On `graft-m3c`, a hot group with **2.626 GB of inline packs above the last `Promoted`** took journey 9's push load (4.17/s offered) through two leader transfers and a kill of the home site's shard. The final run (j9e, 40 min, 7,274 pushes) had **7,273 ok and 1 `ng unpacker error`, with no timeouts**. Push p50 was 3.8 s and p99 19.3 s over the run; it was 7.2 s before the kill and 19.2 s after it. The worst was one 77 s burst at the kill itself. Shard memory per catch-up stayed under 1 MB (`held_max` 954,410 bytes, the most over 16 catch-up reports). `git fsck` is clean on every shard that held the network (see below).

Three changes made the push half work, and all three came from hosted runs, not from the plan:

1. A shard object restarted while its container ran had no inactivity timeout, so the runtime stopped the container about a minute after its last request, on a new disk each time (`577ac5d`).
2. While the home shard rehydrated after a reset (more than 30 min for about 3 GB), every push waited on it until its 180 s deadline. Throughput went to 0 for 40 minutes in j9b. Now the edge validates on another Full site's shard after 15 s on the home (`bf552cb`), and fetches do the same (`ea01beb`). An isolate tries a shard it just saw cold last for 30 s (`2927166`).
3. Promote waits on the home shard. During the home shard's 30 min rehydrate, promote stalls (`promotedTo` stood still from 14:58 to 15:12Z and again from 15:13Z to the end), while archiving stays caught up to it. Promote and archive kept up only while the home shard was warm. That is open; see "Contradictions and open items".

## Blockers 5 to 12, and what is still open

| Blocker (`b0-30.md`) | State on `9a6d955` | Change here |
| --- | --- | --- |
| 5. Whole-tier `{"rows": true}` reads cost about 302 s of CPU on a 4 GB tier | Open: the read stops at `ROWS_MAX` rows only | `1f4ca5e`: the read also stops at `ROWS_BYTES_MAX` (8 MiB) of rows. Test `an_oversized_tier_read_stops_at_its_byte_budget` |
| 6. The first archive's id-block check read every row above it | Fixed on main (`c3a2aa9`) | None |
| 7. The archive drains about 0.2 MB/s, and only through a warm shard | Partly: archive runs one range a step | The promotion validate stays on the first shard and never forwards to a cold pool shard (`derived=1`, `via=0`). `b030-split` owns promote-ahead and the drain budget |
| 8. Catch-up read whole multi-GB gaps into memory | Open | `c8e2770`: catch-up and rehydrate read one `/entries` page at a time, the next page read while one applies, so a shard holds at most two pages |
| 9. Rehydrate read the whole log, not only above the last `Promoted` | Fixed on main (`422bc02`, `19eca55`: the tail's stored packs load in index order) | Kept; the tail now resumes per page |
| 10. Catch-ups outlasted the push's 180 s and the alarm's `RUN_FOR` | Open | `52a2073`: catch-ups run in the shard object under `waitUntil`, not in the caller. A caller waits at most `wait` s, then gets 503 `{"warming": true}`; a warming alarm restarts a cut-off run. A cold pool shard answers a forwarded push at once |
| 11. A cut-off rehydrate restarted from nothing | Open | `c8e2770`: rehydrate saves its place per page with its disk id (`Rehydrating`), and asks the container which packs it holds (`/have`), so a cut-off run resumes on the same disk |
| 12. A short page from a new leader moved the cursor past an entry it never applied | Open (`382ddce` is not an ancestor of B0-30 site day 8, `8753c0a`, checked with git) | The cursor moves only to the last index a page held; a pack missing a base answers 409 and drops the cursor, so the next call rehydrates |
| A31. Derived git work raised `fanout`'s k | Open | `db06534`: `/load` leaves out derived CPU (catch-up, rehydrate, promotion validate) |

Found by this run and fixed:

| Found on `graft-m3c` | Change |
| --- | --- |
| Promote-ahead (`b030-split`) records earlier `Promoted` entries above the last one's range, inside the rehydrate's tail. `rehydrate_plan` left their packs to the tail, so the last promoted pack, which names their objects, never ingested: "1 marked packs have bases no pack provides" on every shard, and archive `/promote` failed | `72ee373`: the marked phase reads the entry of each marker at or above the tail start, 16 at a time, and loads every promoted pack first, by index |
| With no load, each new leader of the 362 MB group spent 30 to 34 s of one alarm's CPU and reset on a storage timeout, so no leader held. `PrSync`'s and `MergeQueue`'s cursors were 0, and each scan read every stored row from there, each decoding its inline packs' base64 | `f4952f8`: both scans stop after `SCAN_BYTES` (16 MiB) of stored rows and re-arm the alarm. base64 decode uses a 256-entry table (4.5x faster, measured on the orb) |

## Design changes

- `design/DESIGN.md`, Shape, `rehydrate` and `IngestCursor`: page-at-a-time catch-up and rehydrate, two pages held, saved per page, resumed on the same disk; catch-ups in the shard object with a bounded `wait`; every promotion still in the log loads first. `NetworkLoad::cpu_p99` leaves out derived work (A31).
- `decisions/DECISIONS.md`: "A git cache shard catches up a page at a time, in the background, and resumes where it stopped".
- `decisions/OPEN-QUESTIONS.md`: G7 and A31 narrowed.
- `design/DESIGN.md`, Shape, `GitCacheShard`: a shard object instance that finds its container running sets the 6 h inactivity timeout itself. `IngestCursor`: pushes and fetches wait 15 s on the home shard, then 60 s on each other Full site's.
- `crates/raft/src` is unchanged, so no long sweep or Porcupine rerun was needed.

## Tests and plants

In memory:

- `crates/git/tests/catchup.rs` (8 tests): a short or empty page never moves the cursor past what was applied; a shard far behind holds at most two pages; a cut-off catch-up resumes after its last page; a cut-off rehydrate resumes and never starts again from nothing (cuts at 5 points); a rehydrate reads only above the last `Promoted`; a disk missing an entry below its cursor rehydrates; a new disk or an archived cursor rehydrates; every promotion still in the log loads before the last one.
- `crates/git/tests/prsync.rs`: a leader far behind reads the stored log a bounded slice per alarm, each entry once and in order, and its batch still names every pull request.
- `crates/api/tests/surfaces.rs`: `an_oversized_tier_read_stops_at_its_byte_budget`.
- `crates/git/src/pack.rs`: base64 refuses every byte outside its alphabet.
- `containers/git/server/load_test.go`: `/load` leaves out derived CPU; `/have`; 409 for a missing named object.

- `workers/edge/src/lib.rs`: `a_validate_falls_back_from_the_home_shard_to_the_other_full_sites` (home first even when the record lists it last, other Full sites in record order, never a Witness, a pinned site alone) and `a_cold_shard_is_tried_last_and_the_rest_keep_their_order`.

Planted bugs (`tests/am1/hotshards/plant.py`, log `tests/am1/raw/hotshards/plants.log`): **23 of 23 caught**. They are 0 to 18 above plus four in the edge's fallback: a fallback to a Witness site, the sites tried in record order rather than home first, a cold shard skipped rather than tried last, and the warm shards reordered by site id.

## Hosted run on `graft-m3c`

Install `graft-m3c` (sites a, b, c, home site a), repository `m3org/hot-a` (`repo:65551`, placement `Tri`). Deploy tree `hs-deploy`: `hot-shards` plus main `4f61b75`, merge-budget-2, lf4-rest-3, and `origin/b030-split`'s promote-ahead. Site day 5 ran at 12:35:16Z from `0603f2c`, with test-faults and image `graft-m1-site-gitshard-git@sha256:7e0c47728cabe0020cd9b3ad3b2fb229f135ba017577aa977e7ec6a3be8905cd`. The final edge is `graft-m3c-edge` `1156f1e2` from `cb5d825` (`--ops graft-m3c-ops`, lf4-rest-3's `FORMATS_ENABLED` with 32.18). Drivers are in `tests/am1/hotshards/` and raw logs in `tests/am1/raw/hotshards/`; `timeline.txt` lists every phase.

**Seed.** 500 KB inline packs at 1.9/s with the archive held (`hold.mjs`), until the least member held **2.626 GB above the last `Promoted`** at 13:10:19Z (log 4.6 GB).

**Runs.** Each run pushed journey 9's packs (10 to 70 KB, 5% at 1 MiB) at 4.17/s offered, 16 in flight, to 500 branches, while `faults.mjs` transferred leadership and killed the leader site's shard (`/_bed/kill`, which tears its container down, so its next catch-up rehydrates on a new disk).

| Run | Edge | What happened |
| --- | --- | --- |
| j9a (10:56–11:50Z) | `14cc4626` | Containers stopped about a minute after their last request: 19 "exited (code 2)" in 90 min, each followed by a rehydrate on a new disk, so pushes stalled. Cause: the inactivity timeout belongs to the Durable Object instance that set it. Fixed by `577ac5d` and site day 5 |
| j9b (13:10–14:17Z) | `14cc4626` | 13:11 to 13:35Z: 6,113 of 6,114 pushes ok at 4.10/s, p50 2.9 s, p99 5.7 s (one 33 s minute at 13:30Z). Archiving drained from 1,249 to 2,446. After the kill at 13:35:53Z, **0 of 177 pushes succeeded for 40 min**: every validate went to site a's shards: the home shard and two pool shards each rehydrated 2.1 to 2.2 GB on a new disk, in 30 to 43 min (`held_max` 2.0 MB). Stopped |
| j9c (14:21–14:28Z) | `9b241b5f` (`bf552cb`) | Validates fell back to site b's shard (0.2 to 2.4 s), but the driver's `ls-refs` (upload-pack) still hung on the rehydrating home shard. Stopped |
| j9d (14:39–14:42Z) | `a4e4e290` (`ea01beb`) | Every push ok, but about 21 s each: 15 s of it was the home shard's `wait`. Stopped |
| **j9e (14:46–15:27Z)** | `1156f1e2` (`2927166`) | Below |

**j9e** (`j9e.log`, `j9e-pushes.jsonl`, `faultse.log`, `monitor.log`):

| | Pushes | ok | p50 | p99 | max |
| --- | --- | --- | --- | --- | --- |
| Whole run, 2,393 s | 7,274 (3.04/s completed) | 7,273 | 3.8 s | 19.3 s | 78.8 s |
| Warm, 14:48 to 14:58:30Z | 2,195 | all | 4.2 s | 7.2 s | 9.7 s |
| After the kill, 15:01 to 15:27Z | 4,706 | all | 3.6 s | 19.2 s | 20.3 s |

- Faults: a transfer at 14:52:31Z (term 48 to 50), the kill of site a's shard at 14:58:33Z, and a transfer at 15:12:31Z (term 50 to 52). The pushes in flight at the kill took about 77 s and all succeeded, with one more burst of 22 s at 14:59:50Z while site b's shard caught up a minute. After that, 11% of pushes (16 per minute, in bursts) took 15 to 20 s. These are the pushes an isolate sent before it had seen the home shard cold, so they waited out its 15 s `wait`. The one `ng unpacker error` was at 14:58:29Z, 4 s before the kill.
- The offered 4.17/s was not reached: 16 in flight at about 4.3 s each completes about 3.6/s, and 3.04/s over the run. This is the driver's in-flight cap, not a refusal.
- Promote and archive: while the home shard was warm (14:46 to 14:58Z), `promotedTo` rose from 4,373 to 4,760 and `archivedTo` followed it to 4,760. The log above `Promoted` fell from 2.757 to 2.588 GB while pushes added to it. After the kill, every promote answered 503 `warming` (the archive alarm calls only its own site's shard), so `promotedTo` stood at 4,760 to 4,769 and the log above it grew to 2.805 GB by 15:27Z.
- Shard memory: `held_max` was at most 954,410 bytes over 16 catch-up reports (2.0 MB in rehydrates). Container memory grew with disk during rehydrates, up to 2.6 GiB of the standard-2's 6 GiB; at idle it was 0.2 GiB. Memory tracks disk to within about 0.1 GiB throughout, so it looks like page cache rather than heap [inference].
- Leader changes: both transfers returned `Ok` and the term rose by 2 each time, but site a was leader again before the driver's next status read 0.2 s later. The leader returned to its home site (`health.rs`, `maybe_hand_off`) [inference: the intermediate leadership was not observed].
- `git fsck` after the run (`fsck-j9e-12.log`): sites b and c were each caught up to index 15,463 with `ok: true`, no dangling or missing objects, and identical refs (504). Site a's shard was still rehydrating at teardown (2.4 GB on disk at 15:32Z) and held no refs yet, so its fsck checked an empty repository.

## Cost

`tests/am1/hotshards/usage.py` for 07:05 to 15:35Z on 2026-11-12: **$10.36** at overage rates. That is Workers and Durable Objects $6.06 (99,979 Workers requests, 1,763 s of Workers CPU, 2.13 M DO requests, 5.21 M rows written, 291,232 DO active-seconds) and containers $4.30 for about 2,000 standard-2 instance-minutes. The container minutes come from the container metrics API, whose 2,000-row cap per query may undercount the morning, so read the container figure as a lower bound. R2 is not included. The spend cap was $40.

## Contradictions and open items

- **Promote stalls while the home shard rehydrates.** The archive alarm promotes only through its own site's shard (`workers/site/src/archive.rs`, `shard()`), and a promotion's pack must come from a shard that ingested every pack in the range. After a reset, the home shard rehydrates the whole tail above `Promoted` (2.1 to 2.8 GB, 30 min or more), so promote stops for that long and the log grows by about 0.25 GB at journey 9's rate. Archiving never passes `Promoted`, so it stops too. The fix belongs in `archive.rs`, which the split thread owns: promote on another site's warm shard as the edge now validates, and place the result as before. I have not edited `archive.rs`.
- **Rehydrate takes 30 to 43 min for 2.1 to 2.2 GB.** That is about 0.8 to 1.2 MB/s, mostly per-pack `index-pack` calls (7,000 to 9,600 inline packs); j9e's home shard was still rehydrating 34 min after its kill. The pushes no longer wait for it, but promote does (above), and so does a fetch whose every shard is cold.
- **15 s per push until an isolate sees the home shard cold.** `COLD_SHARD_MS` lives in each edge isolate, so every new isolate pays the home's 15 s `wait` once per 30 s. That was 11% of pushes in j9e's last 25 minutes and set the p99 at 19 s. A shorter home `wait` or a shard-reported "warming" read before the push would cut it [guess].
- **A transfer away from the home site does not stick.** The home hand-back moves leadership back within seconds, so these runs exercised the transfer and the home's return, not a sustained non-home leader. A leader on site b would make `archive.rs` promote on site b's shard, which is warm, so the stall above would not occur in that case [inference].
- **Container memory tracks disk** (up to 2.6 GiB during a rehydrate). `held_max` shows the shard object's buffers stay at 1 to 2 MB, so the growth is the container's page cache or overlay accounting [inference]. It stays under standard-2's 6 GiB at this size, but a larger tail would approach it.
- **The inactivity timeout is per object instance** (Cloudflare documents `setInactivityTimeout` on the instance). Without `577ac5d`, any eviction of a shard object while its container ran stopped the container a minute later. DESIGN.md now says so.
- **The archive hold during seeding.** The seed held archiving (`hold.mjs`) to build 2.6 GB above `Promoted` in hours, not days. The run starts from a log built faster than production would build it.
- **base64 and leader-scan CPU** (`f4952f8`): each new leader of the 362 MB group spent 30 to 34 s of one alarm's CPU and reset; fixed by bounding scans at 16 MiB per alarm and a table decode.
- G7 stays open on the promote stall. A31 needs a run under catch-up load that shows no `Fanout` step; none of these runs logged one, but none was designed to test it.
