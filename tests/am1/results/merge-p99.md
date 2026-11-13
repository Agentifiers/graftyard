# `merge-p99`: where B0-29's `merge:only` time goes at 500 agents, and the cuts (2026-11-13)

Branch `merge-p99`, from `origin/main` `20147db`, with `origin/lm1-memory` `c7f811b` merged first and `origin/main`
`020b3e5` later. Hosted on `graft-m3f` (`tests/am1/results/install-m3f.md`), `SHARD_INSTANCE=standard-4`, shared with
rehydrate-batch. B0-29's line: `merge:only` p99 at most 2 s at journey 9's 500 agents.

## Summary

**B0-29's merge line passes on `4595d33`.** The 600 s B0-29 row at 500 agents on `graft-m3f` (`mwemk5s2`, standard-4,
leader held at site 0, term 2, no leader change) measured `merge:only` p50 907 ms and **p99 1,571 ms** (n 94: 83 200s,
11 409s "Base branch was modified"), against the 2 s line. On main `20147db` the same shape measured p99 4,163 ms.

- **Instance size alone did not close it.** Main at standard-4 measured p99 4,163 ms, against LM-1's 2,705 ms (300 s smoke)
  and 4,260 ms (600 s row) at standard-2. The slow stages were reads waiting on read-index rounds and a dump rebuild, not
  shard CPU (the shard's own merge was p50 63 to 103 ms).
- **Where the time went:** the merge's tier read (up to 2.8 s, waiting out learner dump rebuilds), an untimed serial `o/r`
  resolve after the token (135 to 705 ms), and then the owner's `Authz` reads by read-index through far members (p50 450 to
  530 ms). The commit (0.4 to 0.9 s, tied to leader CPU) is now the largest stage. Budget wait, catch-up, turn queue,
  and polling cadence are not on this route's path.
- **Three commits:** `8752c1e` (merge rows never wait for a dump rebuild; `o/r` resolves beside the token; per-request
  timing headers), `2f5f25d` (merge rows on the repository leader; reads ahead on the isolate's cached repository, kept
  only if the fresh resolve agrees), `4595d33` (a REST merge's `Authz` reads on the `Authz` leader).
- **Confirmed on main `5293a6c`** (merge-p99 merged with hot-shards-3 and m0-carried-7; site and edge day 07:24Z to
  07:35Z, image `696907a2`): two more 600 s rows on fresh repositories, back to back with no window selection, at p99
  **1,208** and **1,386 ms** (n 83 and 90), each with the leader held at term 2. All three 600 s rows pass: 1,571 /
  1,208 / 1,386 ms. The commit is the largest stage on all three (p50 432 to 532 ms, p99 687 to 871 ms).
- **The 3 s `/raft` lead:** the merge-budget-3 stretches exist on
  `graft-m3f` too (idle system-group followers on sites b and c) but do not line up with merge tails.

## 1. Where the time went before (LM-1's m19 runs on `graft-m2b`, standard-2)

`merge:only` is `PUT pulls/{n}/merge` alone, from request to reply. Its `server-timing` stages, from the raw `mergeSteps`
of LM-1's runs (`a1-b0-29-mwecii86`, `-mwecsudw`, `-mwed6jm6`):

| Run | n | p50 / p99 | `auth` p50 | `authz` p50 / p90 | `tier` p50 / p90 / max | `merge_plan` p50 / p90 | `commit` p50 / p90 | untimed `gap` |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| s250 smoke | 27 | 1,487 / 3,279 | 46 | 298 / 495 | 397 / 955 / 2,229 | 164 / 313 | 462 / 540 | 143 to 590 |
| s500 smoke | 31 | 1,577 / 2,705 | 45 | 133 / 366 | 499 / 1,030 / 1,399 | 138 / 238 | 459 / 563 | 135 to 705 |
| B0-29 row, 600 s | 70 | 1,937 / 4,260 | 45 | 204 / 487 | 711 / 1,715 / 2,825 | 210 / 399 | 539 / 739 | 135 to 624 |

- The **merge's tier read** set the tail: 4 of the B0-29 row's 5 slowest (2.5 to 2.8 s each). The shard's own merge
  plan set the fifth (2,964 ms, once).
- An **untimed gap** of 135 to 705 ms sat in every merge: `total` less `auth`, the longer of `tier` and `authz` (which
  run at once), `merge_plan`, and `commit`. In code, a REST write resolved `o/r` after its token, outside any stage.
- **The commit** (the leader's `MergeCall`) was 0.5 to 1.0 s; LM-1 ties it to leader CPU (r = 0.83 per 10 s bin), which
  m0-carried-7's entry cap targets.
- **Shard timing was missing**: 122 of 128 merges carried no `x-graft-shard-timing`. The edge kept it in an isolate-wide
  thread-local that any concurrent reply cleared.
- **Not the budget, catch-up, or turn queue.** Those are the `PrSync` test merge's path (`/merge` without `method`,
  `graftyard_git::prsync::asked`); `PUT pulls/{n}/merge` asks the shard's `/merge?method=` route, which takes no budget
  and no turn, and merges first, waiting for a catch-up only after a miss (LM-1 round 7). Every shard timing read on
  `graft-m3f` showed `waited` 0 and `catchup` 0 but one (150 ms).
- **Not the edge's polling cadence.** The REST merge holds its commit with `/outcome` (`wait_ms` 10 s) and answers
  when the ticket resolves; nothing polls on a period.
- **merge-budget-3's 10 s gap lead** (the lead's note): no `merge:only` here was over 2.8 s, and every hosted run on
  `graft-m3f` held its leader at term 2 with no step-down, so no tail lined up with a 3 s follower stretch. Not chased.

## 2. What changed

| Commit | Change | Why (measured stage it cuts) |
| --- | --- | --- |
| `8752c1e` | `Repo::heavy_wait` spaces only whole-tier dumps; a merge's rows (`{"merge": id}`, `{"mergeN": n}`, sent with `packed`) answer at once | A merge's rows waited out (learner) or were shed behind (follower) a dump rebuild they never made, and took its slot: `tier` p90 1.0 to 1.7 s |
| `8752c1e` | `rest_repo` resolves `o/r` beside `authenticate` for every method, as a `timed` stage | The untimed gap |
| `8752c1e` | `x-graft-trace` and `x-graft-shard-timing` live in the request's `Timing` | Shard timing lost on 95% of merges |
| `8752c1e` | `a1.mjs` reports `mergeSplit`: each stage's n/p50/p99/max and the untimed `gap` | |
| `2f5f25d` | `tier_rows` reads a merge's rows on the leader (`call_opt`), not by read-index on learners and followers | `tier` p99 1.0 to 1.4 s after the first fix: a read-index round from a far member |
| `2f5f25d` | A REST merge reads its rows and the owner's `Authz` fresh, ahead, for the repository its isolate's cache names, and keeps them only if its own fresh `resolve` names the same repository, owner, and placement record | `resolve` p50 381 to 452, p99 861 to 884 ms, serial before `tier`/`authz` |
| `4595d33` | A REST merge reads the owner's `Authz` (`resolve`'s name lookup and the fold) on the `Authz` leader by `call_opt` (`Ctx::leader_reads`) | `resolve` p50 530, p99 1,033 ms and `authz_ahead` p50 463 ms on the 600 s row at `2f5f25d`, both read-index through far members |

Design first: DESIGN "Follower read" (three sentences) and `decisions/DECISIONS.md`, "`merge-p99`" (five entries,
with Why, Beat, Evidence, and Flip). Evidence tags kept: each new claim is [measured] from the runs below, or
[inference] where a cause is read from the code. No `crates/raft` change, so no long sweep or Porcupine run was needed.

## 3. Hosted on `graft-m3f` (500 agents, B0-29 shape, 300 s smokes and 600 s rows, 8 learners enam 4 / weur 2 / apac 2, k 4)

Each run is a fresh `m3org/ps500*` repository; every run held its repository leader at site 0, term 2 (no leader
change). Driver: `tests/am1/mp99/smoke.sh` (`a1.mjs b0-29`); each run's row (stages, `mergeSteps`, leader terms) is in
`tests/am1/results/merge-p99-data/<run>.json`, with LM-1's m19 rows as `m19-<run>.json`.

| Run (repository) | Build | n | `merge:only` p50 / p99 | `resolve` p50 / p99 | `tier` p50 / p99 | `authz` p50 / p99 | `merge_plan` p50 / p99 | `commit` p50 / p99 | Leader `/commit` p99 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `mwehjjx2` (ps500c) | main `20147db` | 38 | 2,750 / 4,163 | (untimed) | 797 / 2,686 | 297 / 662 | 556 / 927 | 604 / 956 | 853 |
| `mweiav9a` (ps500d) | `8752c1e` | 45 | 1,470 / 2,379 | 452 / 861 | 300 / 1,039 | 277 / 680 | 126 / 256 | 406 / 842 | 520 |
| `mweiquz6` (ps500e) | `8752c1e` | 47 | 1,627 / 2,759 | 381 / 884 | 224 / 1,435 | 395 / 785 | 103 / 185 | 418 / 655 | 667 |
| `mwejsb1x` (ps500g) | `2f5f25d` | 49 | 1,236 / **1,679** | 398 / 784 | ahead 33 / 84 | ahead 451 / 784 | 153 / 277 | 482 / 838 | 717 |
| `mwek3y8y` (ps500h) | `2f5f25d` | 48 | 1,282 / **3,618** | 531 / 2,116 | ahead 47 / 65 | ahead 489 / 804 | 138 / 596 | 495 / 746 | 681 |
| `mwekzl20` (pg600b), 600 s | `2f5f25d` | 90 | 1,152 / **2,517** | 530 / 1,033 | ahead 30 / 193 | ahead 463 / 688 | 136 / 1,347 | 367 / 665 | 611 |
| `mwemk5s2` (pg600c), 600 s | `4595d33` | 94 | 907 / **1,571** | 118 / 306 | ahead 36 / 355 | ahead 50 / 144 | 178 / 797 | 513 / 857 | 691 |
| `mweo1bnh` (pc600a), 600 s | main `5293a6c` | 83 | 838 / **1,208** | 195 / 423 | ahead 71 / 163 | ahead 69 / 179 | 168 / 467 | 432 / 687 | 1,063 |
| `mweolsoh` (pc600b), 600 s | main `5293a6c` | 90 | 944 / **1,386** | 197 / 351 | ahead 74 / 122 | ahead 70 / 171 | 171 / 470 | 532 / 871 | 1,702 |

- **Standard-4 alone did not close the miss.** On main `20147db` at standard-4, `merge:only` p99 was 4,163 ms, against
  LM-1's 2,705 ms (smoke) and 4,260 ms (600 s row) at standard-2. The merge's tier read was still the tail (2,686 ms).
  At this scale, size buys nothing measurable on this path: the slow stages were waits on reads, not shard CPU.
- **After both commits** the read ahead was kept on 47 of 49 and 46 of 48 merges. `tier` and `authz` leave the
  critical path, and a merge is `auth`/`resolve` (about 0.4 to 0.8 s), then `merge_plan`, then `commit` (about 0.5 s).
- **`mwek3y8y`'s p99 is one merge**: 3,618 ms with `auth` 1,539 and `resolve` 2,116 ms at once, and no read ahead (its
  isolate's cache was empty). Its next slowest was 2,029 ms, then 1,783 ms. In a 300 s run with 48 merges, p99 is the
  maximum, so a single slow first read in a fresh edge isolate sets it. The other merges with no read ahead (3 of 4)
  were 1.0 to 1.8 s.
- **What that merge waited on** (Workers Logs, its trace `8bfb8f6e…`, `PUT pulls/78/merge` ending 05:55:07.2Z, edge
  `0bac3f72`, 28 ms CPU): two `registry` rows for the release chain, two `/read`s of `release#1#f0` at once (871 and
  873 ms; its followers on b and c answered `/raft` in 7 to 13 ms), the token (`tokens:482#1#f0`) and name
  (`names:152#1#f0`, `names:708#1#f0`) reads at 1 ms each, the repository read (4 ms), two `authz:7100001` reads on
  site c (433 and 279 ms), the shard merge (153 ms), and the commit (509 ms) [measured]. The token and name reads
  start about 1.4 s into the request, after the release reads, so the tail reads as a first request in a fresh edge
  isolate that waits on the current release before its token and name [inference: the order is read from event
  timestamps, not from a trace span]. The isolate had no cached name, so no read ahead was made.
- **The lead's 3 s `/raft` stretches**: present on `graft-m3f` too. Sites b and c each show about 8 to 9 objects of
  idle system groups (`names:*`, `tokens:*`, `release#*`, all followers `#f1`/`#f2`) taking 3.0 to 3.1 s on `/raft`,
  each repeating every about 8.5 s for the whole run, and none on site a. They do not line up with the slow merges: the
  3.6 s merge's own reads of those groups took 1 ms, and its release followers answered in 7 to 13 ms. Not chased here;
  the pattern is recorded for whoever owns idle-group heartbeats [measured].

## Tests and plants

- `a_merge_rows_read_never_waits_for_a_dump_rebuild` (`workers/site/src/fanout.rs`): a merge's rows by id and by number
  answer their rows at once on a voter's and a learner's spacing right after a rebuild, and the whole-tier read is still
  spaced where it was. Fails on the old check.
- `a_write_keeps_its_ahead_reads_only_for_the_repository_it_resolved` (`workers/edge/src/api.rs`): reads ahead are kept
  only for the same repository, owner, and placement record; another repository under the name, a moved placement, an
  empty or failed resolve, or no guess reads again; a failed `Authz` read ahead is read again.
- `only_a_rest_merge_reads_on_leaders` (`workers/edge/src/api.rs`): only `PUT pulls/{n}/merge` reads on leaders; a `GET`
  or `POST` of the path, a non-numeric number, a shorter or longer path, or `issues/{n}/merge` does not.
- `a1.mjs --selftest`: `mergeSplit` takes the longer of `tier` and `authz`, and adds no shard stage for a merge without
  one.
- Plants in `tests/lm1mem/plant.py`, 6 of 6 caught: 76 (merge rows spaced as a dump again), 77 (whole-tier dumps
  unspaced), 78 (`mergeSplit` counts `tier` and `authz` in series), 79 (a read ahead kept for another repository under
  the name), 80 (kept after the placement moved), 81 (any `PUT` under `pulls/{n}` read on leaders).

## Checks

On `2f5f25d` and again on `4595d33` (main `020b3e5` merged): fmt; workspace clippy; wasm32 clippy for site and edge, with and without
`test-faults`, and for ops; `cargo test --workspace`; `check_design`, `check_names`, `check_links`; `go test` in
`containers/git/server` (`tests/am1/runs/checks.sh`). All pass.

## Coordination and setup notes

- Site-day log rows were sent to rehydrate-batch by message (`install-m3f.md` was not yet on main): merge-p99's site and
  edge day 04:46Z to 04:58Z (sites `7593896a` / `765b16f8` / `3087928b`, edge `3206b6f0`), edge day 05:24Z to 05:28Z
  (edge `386e7b2e`), then rehydrate-batch's site day from 05:33Z, built from rehydrate-batch merged with `2f5f25d`.
- `a1.mjs` on a fresh install needs the install org's site-admin row (`node tests/am1/rest/apps.mjs`) and the AM1
  permissions on `m3-ci` (`node tests/am1/hosted/app-perms.mjs`); both are idempotent and were run on `graft-m3f`.
- merge-p99's edge day 06:53Z to 06:56Z (edge `4c3d1db9`, from `4595d33` merged with rehydrate-batch `c5b2130`, so the
  edge kept its test-faults `/_bed/shard_abort`), then the 600 s row; rehydrate-batch held deploys for both 600 s rows.
- The first 600 s attempt (`pg600a`, 65546) found its repository without `main`: its create hung 180 s just after
  rehydrate-batch's 06:05Z site redeploy and the retry read it back. It ran no load and was discarded.
- One smoke (`ps500f`, 65543) started at 05:33:18Z, as rehydrate-batch's site day began; it was stopped and discarded.

## Cost

`graft-m3f`, install-wide, 04:22Z to 08:09Z (`tests/fanout/m2bcost.py`): **$17.63**, of which the confirmation site day
and two rows (07:24Z to 08:09Z, with m3f to this thread alone) were $4.82. The window before 07:12Z includes
rehydrate-batch's runs on the same install, so merge-p99's own share is lower. It is under the $20 cap.
