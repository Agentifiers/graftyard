# G39's merge tail: a merge's catch-up that stops at its head (design, $0, 2026-11-23, not built)

The lead asked for a price on one lever for G39's remaining gap (`decisions/OPEN-QUESTIONS.md`, G39): a merge's
catch-up that stops at its head's index. It is priced against B0-32 m3fmA's one slow merge and B0-33 erDA's 17
`warming` merges, using the `merge_plan`, `waited` and `catchup` splits, and set against the alternative of home
pieces that take more than one index each. F112's thread (`decisions/OPEN-QUESTIONS.md`, F112) left this item with
B0-32's thread, and is editing only the composed-miss chain code (`ready_chain`, `chain_ready`, the `Compose` impl).
Nothing here touches those.

## Summary

- **The lever is already built.** Since `catchup-pieces` (G34), `merge_then_catch_up` merges again after every piece
  of the shard's catch-up once it has missed, so a merge ends after the piece that brings its commits, not after the
  run (`crates/git/src/merge_cache.rs`, `a_merge_goes_after_the_piece_that_brings_its_commit`). m3fmA's slow merge
  did exactly that. Stopping a catch-up "at the head's index" adds nothing to what it does now [measured, below].
- **What it waited for was the entries before its head.** The home shard applies the log in order. When the merge
  arrived, the home stood at index 365 and the merge's head at about 387. The 1,784 ms it waited was the home
  ingesting entries 365 to 386, one burst entry of 59 packs among them. No catch-up that keeps the log's order can
  skip them [measured, below].
- **The one saving inside this lever is the run boundary:** about 0.07 to 0.22 s. The run in flight when the merge
  missed had read its head before index 387 committed. So the merge waited for that run to end (`waited` 1,309 ms)
  and for a new run to read heads again (`catchup` 475 ms, of which 69 ms was the heads read). Letting a running run
  raise its `until` when a waiter needs more saves that read and the run start. On m3fmA that is 2,703 ms to about
  2.6 s, still over 2 s [inference].
- **Home pieces taking more than one index each do not apply.** A burst entry holds more packs than a piece's cap
  (`catchup::PIECE`, 32 packs), so each such entry is a piece of its own whatever the cap. Bigger pieces only wake a
  waiting merge less often. The total ingest is the same, so a merge waits as long or longer [inference from code
  and the pieces below].
- **erDA's 17 `warming` merges are the home's 150 s lag.** That was the lag before G39's levers A, B and C, so
  neither lever above moves them. G39's levers are priced to close that lag. Their hosted check is still B0-33's row,
  with no B0-33 run yet. m3fmA's home stayed level with the pools (p50 −0.2 to 0.0 s), which fits but does not settle
  it at B0-33's pack rate.
- **Recommendation: build none of these now.** B0-32's merge line is met at 90%, and the lever that would close its
  last 0.7 s is not this one (below).

## m3fmA's slow merge, step by step [measured]

Sources: the a1 merge steps (`tests/am1/results/lm1-rows-m3f-data/m3fmA-raw/merge-steps-b0-32-mwtvuboq.jsonl`), the
merge pushes and samples in the run's a1 files, the leader's commit index from `tests/fanout/raw/tsave-m3fmA-b0-32.jsonl.xz`
(kinds 12 and 19), and the home shard's `shard catch-up stored` lines (`tests/fanout/raw/culag-m3fmA.json.xz`). The
clocks are the client's and Cloudflare's, so the joins carry about ±100 ms.

| Time (Z) | Event |
|---|---|
| 23:18:13.4 to 14.6 | The previous `merge:only` (1,266 ms). Its merge commit is this merge's base; the leader stood at index 364 when it ended |
| 23:18:20.3 to 21.8 | The head's push (`067b561c`) commits; the leader is at index 387 to 390 when it acknowledges |
| 23:18:22.8 | The home finishes piece 364 → 365 (59 packs). The base is on the disk |
| 23:18:23.2 | The `merge:only` starts at the client; the shard sees it about 0.1 s later and misses the head |
| 23:18:24.1 | Piece 365 → 366 (59 packs, 1,260 ms of ingest) |
| 23:18:24.6 | Piece 366 → 372 (15 packs) ends the run in flight, `behind: false` at its `until`. `waited` ends: 1,309 ms |
| 23:18:25.1 | A new run reads heads (69 ms), then piece 372 → 397 (25 entries, 9 packs) brings the head. `catchup` 475 ms |
| 23:18:25.96 | The client has 200 after 2,746 ms (edge 2,703: `merge_plan` 2,063, of which the shard's `merge` was 1,960) |

- Every other merge on m3fmA took at most 1,253 ms. 22 of 24 had `waited` 0 and `catchup` 0; one had `waited` 91.
- The home's pieces in the burst: 94 pieces with stored packs, 43 of them one entry. Fixed laps (`up` plus
  `cursor`) were p50 46 ms a piece, and a heads read p50 76 ms when one ran. Ingest was most of each piece
  (fixed laps are 9.3% of piece time).

## erDA's merges [measured, `a1-b0-33-mwtl5ng2.jsonl`'s `mergeSteps`, from the edge-refusal thread's local raws]

| Merge | Client ms | Status | `merge_plan` | Shard `merge` / `waited` / `catchup` |
|---|---|---|---|---|
| 1 | 2,167 | 200 | 104 | 71 / 0 / 0 |
| 2 | 5,365 | 200 | 4,291 | 4,211 / 947 / 2,974 |
| 3 | 8,829 | 200 | 6,982 | 6,946 / 1,438 / 5,170 |
| 4 | 8,279 | 409 | 6,110 | 6,091 / 5,793 / 0 |
| 5 | 12,759 | 200 | 11,326 | 11,324 / 1,614 / 9,001 |
| 6 | 15,821 | 200 | 14,674 | 14,635 / 4,079 / 9,782 |
| 7 to 23 | 15,024 to 16,265 | 503 | — | `warming` at `MERGE_WAIT` (15 s) |

- `catchup` grew run by run, from 3.0 to 9.8 s, while `waited` stayed 0.9 to 5.8 s. Each merge's head was further
  behind the home's cursor as the lag grew to 150 s. The run boundary is at most one heads read and one run start a
  merge, about 0.1 to 0.2 s here (erDA's heads read p50 144 ms). A run-extension lever would leave all 17 `warming`
  answers in place [inference].
- On erDA the home's one-entry pieces held p50 16 packs, against 32 for a full piece. Folding more entries into a
  piece changes when a merge is woken, not how much the home ingests.

## What each lever touches, and its price

| Lever | Files and functions | Saves on m3fmA's merge | Saves on erDA | Overlap with F112 (G37) |
|---|---|---|---|---|
| **The asked lever: stop at the head's index** | none: built as G34's per-piece retry (`merge_then_catch_up`; `workers/site/src/shard.rs`, the `/merge` route's `turn`) | 0 | 0 | none |
| **Run extension:** a running run raises `until` to the group's head when a waiter needs a run that started after it arrived, instead of ending and starting again | `workers/site/src/shard.rs`, `Jobs::start`'s loop and `Jobs::turn`; `crates/git/src/catchup.rs`, `Run::until` refreshed between pieces (a heads read at the piece boundary) | about 0.07 to 0.22 s (2,703 → about 2.6 s) | about 0.1 to 0.2 s a merge; the 17 `warming` stay | none: F112 is not editing `Jobs` or `catchup.rs` |
| **Home pieces of more than one index** (a larger `PIECE`) | `crates/git/src/catchup.rs`, `PIECE` | 0 or worse: burst entries are 57 to 62 packs, over the 32-pack cap, so they stay one-entry pieces; a larger cap only delays wake-ups | 0 or worse | none |
| **The head from the validate aside** (not asked; the lever that would close m3fmA's tail): on a miss, the merge's quarantine also reads the container's `graft/validated-*` packs as alternates, so a head whose push this container validated merges before the log brings it | `containers/git/server/main.go`, `handleMergeChain` (alternates); `workers/site/src/shard.rs`, the `/merge` route's first try | up to the whole 1,784 ms of `waited` + `catchup`, so about 0.9 s, **only if** this push's validate ran on the home object [unverified: the edge's `validate_site` names site 0, not the object; pool forwarding (`push_validator`) sends about 3 in 4 pushes to the 3 pool shards at k 4, and a1's merge pushes are Human pushes that it forwards too] | the merges whose head the home itself validated; with the base also behind, none | none: F112 is not editing `containers/git` |

Notes on the last row, which is a direction, not a proposal:

- **Correctness rests on content addressing.** A merge reads commits by id, so an aside's objects are the committed
  bytes when the id matches. The tree-diff check still runs on the result. Validate's `index-pack --fix-thin`
  completes an aside against the repository it validated on, and the repository only gains objects until the next
  repack. A missing object makes the merge answer 409 and fall back to the catch-up, as it does now [inference].
- **The base must be on the disk too.** On m3fmA it was (index 364 by 23:18:22.8). On erDA's later merges it was not,
  since the previous merge's commit was 150 s behind as well. Keeping each merge's own produced pack aside would
  cover that half; it is not designed here.
- **It covers about 1 in 4 merges at k 4** unless the merge also asks the pool shard that validated the head, which
  the merge request does not name [inference]. So it would not close the line on its own.

## Flips

- **Build the run extension** if a B0-32 or B0-33 row shows merges over 2 s whose `catchup` is mostly a heads read
  and a run start (`catchup` under 300 ms with `waited` over 1.5 s).
- **Open the aside direction** if B0-33's G39 row still answers `warming` with the home level with the pools, or if
  a gate row's merge line fails on merges whose head this container validated.
- **Drop this item** if B0-33's G39 row passes its merge line: the remaining tail is then B0-32's one merge in 24,
  already inside 90%.
