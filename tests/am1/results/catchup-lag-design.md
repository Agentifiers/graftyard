# catchup-lag: the home shard's catch-up falls behind B0-33's pack rate (design, not built)

F112, 2026-11-23. Asked by the lead after edge refusal's Row B2 (`tests/am1/results/edge-refusal.md`, "The git cache
`warming` 503s"): a $0 design, priced against B0-33's pack rate, with the question as G39 in
`decisions/OPEN-QUESTIONS.md`. No code until the lead rules. Tags follow `design/SHARED.md`, "Read this first".

**Short answer.**
- **The home shard has about two thirds of the rate it needs.** On erDA and mrB, site a's pool shards took B0-33's
  staged packs at 18.6 to 20.0 a second of catch-up wall, and the home shard at 11.8 [measured]. The load offered 18 a
  second on average (erDA: 5,404 stored packs between load start and the pools' end), and 19.2 in its busiest minute.
  So the home fell behind by about 6 packs a second: 150 s behind the pools by the load's end on erDA, 88 s on mrB.
  A merge whose head was in that gap waited out `MERGE_WAIT` (15 s) and answered `warming`.
- **Two per-piece repeats cost every shard 20 to 34 ms a pack, and the home the most.** Since `catchup-pieces`
  (2026-11-20), each piece is one `catch_up` call with a fresh `Tally`. So it calls `make_room` (the container's
  `/repos` walk of the whole disk) once per piece, not once per `ROOM_EVERY` bytes: 16 ms a pack on the home, 3 to 4
  on the pools. It also drops the rest of the log page it read, and reads the page again at the next piece's start,
  with no read-ahead: 17 to 21 ms a pack on every shard.
- **The rest of the home's extra is its per-pack ingest:** 48.5 ms a pack against the pools' 27 to 33. Each stored
  pack goes in alone (`ingest_spooled`), and the home container's `index-pack` takes 25.9 ms against the pools' 14.
  The home container also runs the network's validates (3,748 in two minutes of erDA's load) and every merge.
- **Proposed: three levers, A + B + C** (below), priced together at about 33 to 38 ms a pack on the home. That is 26 to
  30 packs a second, 1.4 to 1.6 times the busiest minute's rate [inference from the measured parts; C's share is a
  guess]. A and B alone reach 19.7 a second, about the busiest minute's rate and no margin, so they need C. Past C
  the bound is the stored packs' fetch, 33 ms a pack on the home at `FETCH_AT_ONCE` 4.
- **Not proposed:** a larger `k`, routing merges to a pool shard, or merges reading the leader's tip (below). None of
  them makes one shard's disk catch up faster.
- **B0-34:** all three levers sit on paths B0-34 uses (the monorepo catch-ups behind validates, and the kill phase's
  catch-up). They only remove repeated work. They do not touch maint-yield, the hedge or the gate, so they can't
  move B0-34's lines the wrong way [inference]. B0-34's own pack rate is far below B0-33's.

## The measurement

Read from site a's `shard catch-up stored <net>:` lines in Workers Logs, with each object's name from
`durableObjectsPeriodicGroups` (`tests/fanout/culag.py`; raws `tests/fanout/raw/culag-erDA.json.xz` and
`culag-mrB.json.xz`). These are pieces that ingested stored packs, from load start to each object's last piece.
erDA (net 131404, load 18:21:20Z) [measured]:

| Shard | Pieces | Packs a piece | Wall a piece (ms) | `room` | Page read | `ingest` | `refs` | Wall a pack (ms) | Packs/s |
|---|---|---|---|---|---|---|---|---|---|
| `git#0#net:131404` (home) | 277 | 17.8 | 1,506 | 286 | 311 | 865 | 44 | 84.4 | **11.8** |
| `git#0#pool:2` | 314 | 17.2 | 920 | 57 | 291 | 563 | 9 | 53.4 | 18.7 |
| `git#0#pool:5` | 318 | 17.0 | 910 | 74 | 361 | 465 | 10 | 53.6 | 18.7 |
| `git#0#pool:6` | 315 | 17.2 | 924 | 73 | 363 | 479 | 10 | 53.9 | 18.6 |

mrB (net 131396, load 13:15:50Z): the home took 85.0 ms a pack (11.8 a second), and pools 1, 4 and 7 took 49.9 to
50.8 (19.7 to 20.0) [measured]. "Page read" is the piece's wall less its four page laps (`room`, `ingest`, `refs`,
`save`). That is the log page reads the piece waited for, so it is [inference] that nothing else is in it.

- **All 277 home pieces called `make_room`,** and its cost grew over the run: 69 ms a piece over the first 50 and 294
  over the last 50 (mrB: 49 to 308). The container's `/repos` walks every file on the disk (`dirBytes`), and the
  home's disk grew by every ingested pack and every validate's kept aside [measured counts; the cause of the growth
  is an inference].
- **The home's lag** behind the first pool shard to reach the same log index grew about 0.5 s a second, from 0.2 s
  at +0 s to 131.7 s at +300 s and 148.6 s at +390 s (mrB: 87.2 s at +300 s) [measured]. The home finished at
  18:28:36Z, 70 s after the pools and 136 s after the load ended.
- **The ingest lap's split** (per stored pack: home, then pool 5): container total 34.7 and 18.9 ms, of it
  `index-pack` 25.9 and 14.7, the lock 2.9 and 1.0 [measured]. The rest of the lap, 13.8 and 8.5 ms, is the call's
  round trip and the fetch the in-order ingest still waits for [inference].
- **How many stored packs the home adopted** from its own validate's aside, with no `index-pack`, is not recorded:
  `StoredMs` has no adopted count, so the 25.9 ms average is over adopted and indexed packs together [unverified].

## The levers

| Lever | What changes | Home, ms a pack | Packs/s | Where |
|---|---|---|---|---|
| Today | | 84.4 | 11.8 | |
| **A. Room once per run** | A run's `make_room` budget (`Tally::room`) carries across its pieces, so the walk runs once per `ROOM_EVERY` (256 MiB) of the run, as written. A piece of 0.8 MB pays it about once in 300 pieces | 68.4 (−16.0) | 14.6 | `crates/git/src/catchup.rs` (`catch_up` takes and returns the budget); `workers/site/src/shard.rs` (`Jobs::start` keeps it between pieces) |
| **B. Keep the page across pieces** | The entries a piece truncated stay with the run, and the next piece starts from them; the read of the page after that overlaps the piece, as it does inside one piece today. At most one page waits between pieces, inside the "two pages" memory rule | 50.9 (−17.5) | 19.7 | the same two files; `follow` returns the unapplied tail |
| **C. Batch the stored ingests** | A run of spooled stored packs goes to the container in one call (a new `/ingest-spooled-batch`, the spooled form of `/ingest-batch`), concatenated and indexed once with the same split-on-failure rule, adopting any aside first. It takes the per-pack round trip and `index-pack`'s per-call cost out | 33 to 38 [guess] | 26 to 30 | `containers/git/server/main.go`; `crates/git/src/catchup.rs` (`ingest_all`'s stored branch, as the inline branch batches today); `crates/git/src/host.rs` |
| A + B + C | | 33 to 38 | 26 to 30 | one site day (the image and the site script) |

- **A is a regression from `catchup-pieces`**, which did not keep the `ROOM_EVERY` budget across calls. It is the
  cheapest lever, and it carries no correctness risk: `make_room` only evicts other networks [inference from
  `evict_plan`, which never evicts `keep`].
- **B's risk is a stale tail.** The tail is entries the committed log already holds, so a tail kept across pieces is
  as valid as one kept inside a piece. The cursor still moves only past what was applied. A run that ends, or an
  isolate reset, drops the tail, and the next run reads again from its cursor.
- **C's price is a guess, and the fetch bounds it.** `/ingest-batch`'s concatenation measured 2.8 to 17.8 s for
  9,000 packs locally, 0.3 to 2 ms a pack (`tests/am1/results/hot-shards-2.md`), so the batched ingest itself would
  be 5 to 12 ms a pack on the contended home [guess]. But the ingest then waits on the fetches: the home's spools
  spanned 33.1 ms a pack (pools 18.6 to 27.9), four at a time at 96 ms each [measured, `StoredMs::spools`]. So
  A + B + C lands at the fetch plus the refs, about 33 to 38 ms a pack [inference]. A fifth or sixth spool at once is
  not available: `FETCH_AT_ONCE` already leaves only the ingest's and the page read's connections. The hosted check is the next B0-33 row's home `index_ms` and
  `ingest` laps. Stored packs keep their own `.objs` records by offset, as `ingestRun` writes them for inline runs, so
  `/promote` and `/have` behave as today.
- **Projected effect on B0-33** [inference]: at 26 to 30 packs a second, the home takes the busiest minute's 19.2 with
  7 to 11 packs a second to spare, so a burst's lag drains instead of growing, and the home stays within a few pieces
  (a few seconds), not 150 s. Merges should wait at most a
  piece, well inside `MERGE_WAIT`, and the 17, 9 and 6 `warming` merge 503s go. Human pushes stop meeting `warming`,
  so a1 stops skipping human pushes at the cap.
- **B0-32** (m3fk2B: 3 of 22 merges over 2 s, `merge_plan` waited 6.1 and 1.8 s plus about 7 s of catch-up): the
  same levers cut a burst's catch-up from 45 to 86 ms a pack to about 33 to 38. A merge that needs the burst's last
  entry still waits for the whole burst: 500 packs at 33 to 38 ms is 16 to 19 s. So the 2 s merge line on B0-32
  is not fully settled by this, and needs its own row after it [inference].

## Considered and not proposed

- **More pool shards (a larger `k`) on the home site.** Every shard holds the whole network and ingests every pack,
  so a larger `k` adds shards that each catch up at their own rate, and merges stay on the home. It moves validates off the home container,
  which would bring its `index-pack` toward the pools' 14 ms; C saves more of that, and a larger `k` costs a container [inference].
- **Routing merges to the most caught-up shard.** The pools were behind too: they finished 66 s after the load ended,
  since 18.7 a second barely covers 18. It also changes the merge's home-shard contract in the agent block's merge
  path for a gain A to C already give [inference].
- **Merges reading the leader's tip instead of waiting.** The merge needs the head's objects on the disk; a tip
  names a commit but brings none of its objects, so this cannot remove the wait.
- **Larger pieces.** Pieces exist so a held validate or merge gets a turn (`catchup-pieces`). B gets the larger
  pieces' read savings without the longer turns.

## What to record if built

- `StoredMs`: `adopted`, the stored packs moved in from an aside. That settles the question above, and C's share.
- A `Report` field: `room` calls per run, so a row sees A hold.
- A plant each (numbers from the lead): A, a budget reset each piece; B, a tail kept past the cursor, or applied twice;
  C, a batch that records one pack's objects under another's sum.
- Hosted check: one B0-33 row through a site day, with the home's packs a second, its lag, and the `warming` merge
  503s next to erDA's. Then B0-32's merge line.
