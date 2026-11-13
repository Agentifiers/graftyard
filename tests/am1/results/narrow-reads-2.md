# narrow-reads-2: identity and name reads at once, cached per isolate; a journey 9 API target

Branch `narrow-reads-2`, from main 2db78d2 (which carries `narrow-reads` as 6d23e53). Decision record `decisions/DECISIONS.md`, "`narrow-reads-2`"; F65. Hosted on `graft-m3c`, 2026-11-12 to 13.

## Summary

After `narrow-reads`, journey 9's API reads were served by learners at 99.8% but p99 was 4.4 to 4.9 s. This branch first measured where a request's time goes, then cut the reads before the tier read: the token and `o/r` are read at once, `o/r` resolves in the `Authz` fold the gate needs anyway, and the token row, `ghs_` keys and `jti`, login, `Placement`, `Authz` fold, and account logins are cached per isolate for `READ_CACHE` (10 s) on read-class requests only, keeping only answers that let a request through.

Same install, repository shape (500 open PRs, dump 1.09 to 1.10 MB), 8 learners, and 300 s of B0-29 at 500 agents:

| `read:api` (500 agents, 200 reads/s) | Before (edge ebd77ab4) | After, run 2 (edge 3dd873b0) | After, run 1 |
| --- | --- | --- | --- |
| 200 | 57,027 | 58,283 | 56,215 |
| 503 | 133 | 63 | 1,886 |
| Skipped at the driver's 400 in flight | 1,575 | 1,020 | 1,237 |
| Served, of answered | 99.8% | 99.9% | 96.8% |
| **p50 / p99** | **1,515 / 4,865 ms** | **655 / 3,152 ms** | 749 / 3,303 ms |
| Edge CPU per answer, GraphQL, p50 / p99 | 10 / 91 ms | 3 / 87 ms | 3 / 94 ms |
| Edge CPU per answer, `GET /issues`, p50 / p99 | 12 / 76 ms | 5 / 74 ms | 5 / 75 ms |
| Edge wall, GraphQL, p50 / p99 | 1,479 / 4,932 ms | 633 / 3,732 ms | 736 / 3,802 ms |
| Edge wall, `GET /issues`, p50 / p99 | 1,525 / 4,956 ms | 637 / 3,408 ms | 733 / 3,491 ms |
| Leader change during the load | none (term 2) | none (term 2) | one (term 2 to 5, site 0 to 2) |

**The gate is not met.** p99 fell from 4.9 to 3.15 s and p50 by 2.3 times, against the new target of 2 s. The per-stage timing now says why: on the after run's sampled requests (1 in 100, `tests/nr2/stages.mjs`), `auth`, `resolve`, `authz`, and `logins` are 0 ms at p50, and the tier read is 611 ms p50 and 2,589 ms p99 for `GET /issues` and 629 / 2,163 ms for GraphQL. The edge's identity reads no longer set the p99; the learners' narrow answers do. `resolve` still shows a tail (p99 630 ms REST; 5.8 s on one GraphQL sample), from isolates whose cache is cold: Workers spreads 200 reads/s over many isolates, and each fills its own.

## The gate

DESIGN had no API latency target for journey 9, and B0-29 and B0-31 had no API line. The design now states one (`design/DESIGN.md`, "Follower read" and journey 9, item 1) and B0-29's row carries it (also `build/BUILD-PLAN.md`): **API read p99 at most 2 s with at least 99% served; Fail over 5 s**. Grounds: GitHub's API gives up on a request at 10 s and answers a GraphQL timeout with 502 or 504 (`research/wire-census.md`, item 22); journey 9's agents poll their PR every 5 s, so a later answer overlaps the next poll; B0-18's learner read p99 of 1 s is the member's share, which leaves 1 s for the edge [inference].

## The change

Commits on `narrow-reads-2`: 22c835a (design, decisions, F65), 8e44360 (the edge change, tests, plants, the stale probe), 21fb1ca (`a1.mjs` keeps each read's `server-timing` stages), 57bf5c1 (timing per request, not per isolate; `tests/nr2/stages.mjs`), then these results.

- **Measured per stage.** Every API request records `auth`, `resolve`, `tier`, `authz`, `logins`, `render`, and `total` in its own `Ctx` (`api::Timing`), sent as `server-timing` on test installs, and 1 request in 100 logs them as an `apiTiming` line on every install (`tests/nr2/stages.mjs` reads them from Workers Observability). The first deploy kept them in one thread-local list per isolate, which concurrent requests reset (the before run's `auth` p50 of 25 ms against 1,515 ms total); the before run's split below is therefore the old edge's `server-timing`, which had the same flaw, and is not used.
- **Parallel.** A REST `GET` reads its token and its `o/r` at once, as GraphQL already did. A `ghs_` token reads its verifying keys and its `jti` row at once: the `jti` is in the claims before the signature is checked, and a failed signature refuses the token whatever the row says.
- **One read fewer.** `o/r` resolves in the owner's `Authz` fold, which the read gate reads anyway (`crate::resolve`); only a name the fold does not hold goes to the `{"repo": name}` read.
- **Cached per isolate** (`workers/edge/src/rcache.rs`). For a REST `GET` or GraphQL query only (`Ctx::read_class`): the stored token's row, the `ghs_` keys, the `jti` row, the login row, the `Placement` row, the owner's `Authz` fold, and accounts' logins, each up to `READ_CACHE` (10 s) from when its read started. Values, never promises (F65). Each cache keeps only answers that let a request through: a revoked token row or `jti` is never stored, a key that fails to verify reads the keys again, and a read the cached `Authz` would refuse reads it fresh before it answers 404. Writes never read the caches and refresh them.
- **Access rows** already ride the narrow answer (`TierRows::access_rows`), so the repository costs one member round trip; nothing changed there.

No wire enum, member route, or site script changed: the deploy was edge-only. No GHES reply changes: every change is in which reads run and when.

### Invalidation (stated in DECISIONS)

A new token, repository, key, or grant serves at once. A revocation of a PAT or `jti`, a removed membership or grant, a rename's old name, and a moved placement may serve API reads for up to 10 s, the bound SHARED's "Token revocation" deviation and journey 4 already give for reads; writes see each at once.

## Tests and planted bugs

- `workers/edge/src/rcache.rs`: 5 tests (TTL from the read's start and one millisecond past it; writes never read and do refresh; an older answer never replaces a newer one; only passing answers kept, through `through`; revoked token rows and `jti`s never kept; the cap).
- `python3 tests/nr2/plant.py`: **12 of 12 caught**: a write reading the cache, the TTL ignored, the TTL one second long, an older answer winning, every answer kept, a revoked token row kept, a revoked `jti` kept, `through` taking the cache for a write, a write's answer not refreshing, an entry stamped on arrival rather than at the read's start, the cap never clearing, and an update clearing the map.
- `tests/nr2/stale.mjs`, hosted on the new edge (`tests/nr2/raw/stale3.jsonl`), with the caches warmed before each change:
  - a repository created under a warm owner: REST 200 and GraphQL data at once;
  - m3-member removed from m3org (whose base permission is Write) stops reading a private repository after 2.3 s; added back, 200 at once; removed again, 404 after 1.8 s, and 404 on 10 more reads;
  - a PAT revoked on its `Tokens` shard: a write 401 at once, reads 401 after 77 ms;
  - `DELETE /installation/token`: reads 401 after 90 ms.
  The first two passes of the probe found its own mistakes, not the edge's: a `SetGrant` on an org member changes nothing (membership already grants Write), and a removal must be waited out before the "opens at once" step (`stale.jsonl`, `stale2.jsonl`).
- Existing narrow tests (`crates/api/tests/narrow.rs`, `tests/nr/plant.py`), `agent_bytes`, and the trust view's `surfaces.rs` byte-diff pass unchanged: replies are not touched.
- Full checks on 21fb1ca: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, wasm32 clippy for site and edge with and without `test-faults` and for ops, `cargo test --workspace` (76 suites, 0 failed), `go test ./...` in `containers/git/server`, and the three `scripts/check_*.py` (`check_links` with the untracked `.worktrees/target-rows` build directory excluded). The timing commit was rechecked with edge clippy (native and wasm32, both feature sets) and the edge's tests.

## Runs

Each run follows `tests/nr/run.sh` (learner-dumps' setup): a fresh repository with 500 open PRs, 8 learners (enam 4, weur 2, apac 2), then `a1.mjs b0-29 --seconds 300 --as-admin`. Sites were hot-shards-2's `hs2-deploy` 4614375 throughout (image sha256:15f6ed1b…); only the edge changed.

| | Before | After, run 1 | After, run 2 |
| --- | --- | --- | --- |
| Edge | ebd77ab4, hot-shards-2's `hs2-deploy` 799c69f | 3dd873b0, `nr2-deploy` d0bd005 (hs2-deploy 4614375 + narrow-reads-2) | same |
| Repository | m3org/nr2b1, repo:65561 | m3org/nr2a1, repo:65568 | m3org/nr2a3, repo:65571 |
| Dump at load start | 1,097,072 B at index 1080 | 1,089,330 B at index 1144 | |
| Load window | 23:04:48 to 23:09:48Z | 23:45:52 to 23:50:52Z | 00:27:17 to 00:32:17Z |
| Rows | `tests/nr2/rows/a1-b0-29-nr2before.jsonl` | `tests/nr2/rows/a1-b0-29-nr2after.jsonl` | `tests/nr2/rows/a1-b0-29-nr2after4.jsonl` |

`python3 tests/ld/summary.py <rows>` prints `read:api`, `read:ref`, pushes, and (after) the stages from `server-timing`. Edge CPU and wall: `node tests/nr/telemetry.mjs graft-m3c-edge FROM TO PATH`. Stages from the sampled log: `node tests/nr2/stages.mjs graft-m3c-edge FROM TO`.

**After run 1 is not the result.** Its leader moved from site 0 to site 2 mid-load (term 2 to 5, 312 failed member calls), and the 1,886 503s are reads every member shed during that, so I ran a second fresh repository. Its own numbers are in the third column; even so its p99 (3.3 s) is close to run 2's.

**Same runs, the rest of the row:**

| | Before | After, run 1 | After, run 2 |
| --- | --- | --- | --- |
| Agent pushes ok, p50 / p99 | 1,239 of 1,239, 3,989 / 9,179 ms | 1,223 of 1,236, 4,997 / 24,493 ms | 1,218 of 1,251, 11,447 / 120,001 ms |
| `read:ref` served, p50 / p99 | 100%, 1,987 / 6,354 ms | 99.0%, 2,643 / 15,292 ms | 98.5%, 5,108 / 77,358 ms |
| Leader commit p50 / p99 | 476 / 1,154 ms | 399 / 1,984 ms (27 samples) | 518 / 1,309 ms |
| Learners' reads shed, waited for a rebuild | 14 to 71; 660 to 4,427 | 4 to 66; 576 to 3,768 | 5 to 104; 448 to 3,709 |

The git path got slower across the window (before, then the two after runs, in that order): agent push p99 9.2, 24.5, and 120 s, and git upload-pack wall p99 on the edge 6.3, 10.2, and 17.2 s. This change does not touch it: git requests never set `Ctx::read_class`, so every cache read there is a miss and the resolve, token, and gate reads are the same reads as before (`crate::resolve` keeps its name read for them). I did not find the cause; the shard containers on hot-shards-2's new image and the order of the runs are the untested candidates. It does not change the API result, which is from separate reads at the same rate.

**NEAR_RTT.** Every repository create this window succeeded on its first try (repo:65561 to 65571), on main's keep-the-nearest placement. One create failed with 422 "name already exists" after an earlier try had committed it, and that repository (nr2a2) had no `main`; it was torn down and the run repeated on nr2a3.

## Cost

Hosted spend in this thread: **$3.32** (`INSTALL=graft-m3c python3 tests/fanout/m2bcost.py 2026-11-12T22:44:00Z 2026-11-13T00:36:00Z`, containers $0.69 included), under the $25 cap. No design cost moves: the caches cut edge-to-member reads per API request (from four or five identity and name reads to none on a warm isolate), which `scripts/cost.py` prices as 1.5 Durable Object requests per read without splitting identity reads out, and edge CPU per answer fell from 10 to 12 ms to 3 to 5 ms at p50 against `CPU_MS["api"]` of 4 ms already in the script. So the cost scripts were not rerun.

## F67

Every fresh group this window again came up with three followers at term 2 and no leader after setup, and learner placement failed "no leader" until a test-only `/campaign` on site 0 answered `resumed` (repo:65561 at 23:01:14Z; repo:65568 at 23:42:34Z; repo:65571 at 00:24:03Z). Status snapshots: `tests/nr2/raw/f67-*-status.json`. In repo:65561, site 1's configured voter was generation 2 (`#2#f1`, re-placed) while the member answering for site 1 was the old `#1#f1` at commit 1. Sent to f67-liveness.

## What is left

- **The gate.** API p99 is 3.15 s against 2 s. The learners' narrow answer is now the p99 (tier 2.2 to 2.6 s at the edge). Their `/stats` show 448 to 4,427 reads each that waited for an answer build and 428 to 3,461 `late` per learner; the next step is to time a learner read's read-index wait, its `NARROW_EVERY_MS` (200 ms) spacing, and its build, at 200 reads/s, and either answer from the last build within `FOLLOWER_WAIT` or add learners.
- **Cold isolates.** Each Workers isolate fills its own cache, so `resolve` still has a 0.6 s tail at p99. Fewer, longer-lived isolates or a cache across isolates (which the design does not have) would cut it.
- **The git path's slowdown** across the window, above, is unexplained.
- F67: three fresh groups out of three needed a `/campaign`.
