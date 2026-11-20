# b034-load: Load34, the hosted B0-34 client (2026-11-20)

Branch `b034-load`, from main f413f2ac (b034-midx merged). Harness only: no site, edge or container script changes, so
the next row needs only its granted site day for midx-background and chain-open, and `graft-m1-load` deployed once.
Design record: `decisions/DECISIONS.md`, "`load34`". The 90% reading of the every-clone line: `build/BUILD-PLAN.md`,
"B0-34's every-clone line and the 90% rule".

## What was built

- **`tests/am1/rest/wire.mjs`**: a1's pack builder, pkt-lines, receive-pack body and result, journey 9's plan and
  `cloneKeyHex`, moved out of `a1.mjs` unchanged, so a1 on the orb and Load34 in workerd send the same bytes. Deflate is
  vendored pako 2.1.0 at level 1 (`tests/am1/rest/vendor/`), because workerd's `node:zlib` writes other bytes at
  level 1: 3,071 against Node's 2,459 for the test's fixed input [measured]. pako matched Node's zlib on 255 of 255
  inputs [measured], and a1's bytes for the fixed inputs are unchanged.
- **`tests/am1/load34/load34.mjs`**: the `Load34` Durable Object class of the test Worker `graft-m1-load`.
  - Each client runs its share of a1's B0-34 load from its alarm: agent pushes, side commands, ref and API reads, and
    feed polls straight to the members over the sites' `Member` bindings.
  - The clients with a role run a1's capped kinds: `tip:move` from staged packs, clone probes, git-shaped human pushes,
    and the hot networks' pushes and stock whole clones. Hot clones are read at 8 MB/s [guess], so they stay slow readers.
  - Samples, pushes, misses and feed events go to R2 every 60 s and at the end.
- **`plan.mjs`**: the split over N clients (`clientPlan`, `roles`), phases, and the six-fetch `Slots`.
- **`driver.mjs`**: the control calls, `stage` (each future commit's thin pack built with `git pack-objects` and put
  under `tip/<history hash>/<i>`), and `cpu` (each client's CPU and active time from `durableObjectsPeriodicGroups`
  by object name, divided by its own request count).
- **`a1hosted.mjs`, `a1.mjs --hosted N [--hints ...]`**: a1 keeps setup, the kill, k, leader reads, fill checks and the
  row. It starts the clients and reads their progress every 15 s, then merges their R2 output into its usual samples,
  pushes and row files, with a `hosted` summary per client (colo, slot waits, timer lag, requests, errors).
- **`tests/fanout/b034.sh` with `HOSTED=N`**: it stages the packs before the row, runs `clonelines.py` after
  `outages.sh`, and writes `load34cpu-<tag>.json`.
- **`tests/fanout/clonelines.py`**: the two re-based clone lines, hits and misses apart per phase on both clocks, as
  measured and with harness outages removed. a1 on the orb now also writes `read:cloneTtfb:{hit,miss}:edge`.
- **`deploy.sh`, `config.mjs`**: they create the `graft-m1-load` bucket and deploy the script with `MEMBER_0..2` bound to
  `graft-m1-site-{a,b,c}`'s `Member`. The bearer token is written to `$M3_STATE/load34.token` (mode 600) and never
  printed.
- **The flake** (`TestConcurrentValidatesOfOneNetwork`): the test's writer posted `/ingest` with no body. When it caught
  a concurrent validate between `discard` and rename, adopt found no aside and the empty body failed its checksum (422).
  The writer now posts the pack's bytes, as a real catch-up does. Unchanged main failed 5 in 250 runs; the fix passed 250
  of 250 (`go test -count=250`, 224 s).
- A stale plant anchor in `tests/am1/rest/plant.py` ("a1: an ng reply reads as ok") was repointed. It already failed
  on main, because `pushResult` reads pkt-lines now, and it now catches its mutant again.

## Measured on Cloudflare before building (`graft-m1-load` probes, 2026-11-20, deleted after)

| Probe | Result |
|---|---|
| 30 fetches of a 2 s answer from one object | 10.1 s (6 at a time) |
| the same from an alarm handler | 10.0 s |
| 5 objects × 30 at once | 10.1 s each, 10.3 s in all (per object) |
| 60 from 10 objects | 2.5 s |
| 30 RPCs to another object (2 s each) | 2.2 s (not held to 6) |
| 60 `stub.fetch` to another object | 2.2 s (not held to 6) |
| 12,000 subrequests from one object (`subrequests` limit raised) | all ok, 22 s |
| fetch loops from an RPC, from an alarm that returned, from one that awaited | ran 25 min, 20 min and 6 min until the probe was stopped; no eviction |
| `node:zlib` level 1 in workerd | other bytes than Node's |

## Tests

`node --test tests/am1/load34/load34.test.mjs` (5 pass). The workerd case needs `WRANGLER_CLI` in a worktree without
tests/m0's npm install.
- The pack bytes for two fixed inputs (a 40 KB blob on a parent, and a root commit with nested trees). They are checked
  in Node and in local workerd against hashes from a1.mjs's builder at f413f2ac, taken before `wire.mjs` existed.
- The split at 23, 40 and 96 clients:
  - every agent and poller exactly once, and the uncapped rates summing to a1's;
  - each capped kind's cap and rate summing to a1's;
  - each hot push client keeping its own refs, and no client holding more than 6.
- Phases at each mark's first millisecond.
- The CPU line per client and in all.
- A 12 s run of 15 real clients in local workerd, against a fake edge and fake members:
  - samples in a1's format, with every gated kind and the phases;
  - `tip:move` sending the staged pack's exact receive-pack body from the advertised main, mid-history, after the
    advertisement;
  - hits and misses split on both clocks, with the misses' CloneKey digest;
  - a slot wait kept out of the samples: the API client waited and its `read:api` p95 stayed under 235 ms at a 120 ms
    edge;
  - feed events joined to their pushes across clients;
  - Basic auth on git and `token` on the API, no token in the output, and every object idle after `/forget`.

`python3 -m unittest tests/fanout/test_clonelines.py` (4 pass): which hits count, the share per cold phase, 5 s itself
counted within, an empty phase failing, and the two clocks kept apart.

Plants (`tests/am1/load34/plant.py`): 19 of 19 caught. On the first run, the plant "tip:move from the history's start"
was missed, because the test's main sat at index 0, where both rules send the same pack. The test now starts
mid-history, as the rows do, and catches it. `tests/am1/rest/plant.py` 23 and 24 and `tests/fanout/plant.py` 27 (a1's
pack builder and push result, now in `wire.mjs`) are all caught.

## mt7 against the new lines, from `clonelines.py`

Client clock (mt7 ran on the orb, before the edge-clock split, so the edge column is empty):

| Phase | Hits n, p50, p99, max | Misses n, p50, p99, max | Over 5 s |
|---|---|---|---|
| j9 | 239, 2.17 s, **5.19 s**, 5.78 s | 70, 3.22 s, 9.41 s, 9.41 s | 3 hits, 10 misses |
| kill | 97, 1.18 s, 3.37 s, 3.37 s | 25, 2.13 s, 10.37 s, 10.37 s | 6 misses |
| second | 151, 2.48 s, 5.04 s, 6.58 s | 31, 2.67 s, 7.66 s, 7.66 s | 3 hits, 3 misses |
| hostile | 230, 1.91 s, 4.40 s, 5.45 s | 82, 2.84 s, 10.33 s, 10.33 s | 2 hits, 22 misses |

- **Hits p99 ≤ 1 s in steady state: fail**, 5.19 s.
- **Every clone ≤ 5 s through kill and cold, at ≥ 90%: pass**, with kill at 95.1% and second at 96.7%. As written
  (100%) it fails.

## Spend estimate for a hosted row

Priced from mt7's measured load (`a1-samples-b0-34-mwo4va3y`) and `research/prices.json`:
- **Requests in flight.** Mean concurrency was 742: 478 feed polls, 233 shared requests (API, ref reads, agent pushes,
  side commands), and 20 on the roles. With 6 slots per client at about half use, the shared load needs about 78
  clients, plus 22 role clients: **N = 100** [inference].
- **Duration.** 100 × 0.128 GB × (900 s window + 90 s lead + 240 s tail + 60 s collect) = 16,500 GB-s, **$0.21** at
  $12.50 per million GB-s. A client's duration bills while its fetches keep it in memory, which is the whole window.
- **CPU.** 250,198 operations at 0.5 to 2 ms is 125 to 500 CPU-s, **$0.003 to $0.01** [guess]. `driver.mjs cpu`
  reports it per client after the row.
- **Requests, R2 and staging.** About 2,000 control and progress RPCs and 2,000 chunk writes, about **$0.01**. The
  edge's own requests are billed as today, since the same load arrives.
- **Container egress.** Hot whole clones at 8 MB/s for at most 120 s are bounded at mt7's orb rate, so container
  txBytes stays near mt7's [inference].
- **Client total: about $0.23 a row, against the $0.06 I scoped.** That scope assumed 24 clients. The six-fetch limit
  measured here needs about 100, and duration is the whole difference. A row is then about **$3.9** on the run meter
  (mt7 was $3.68), inside today's $2 to $5, so the operator's approval threshold is not crossed [inference]. The CPU
  read on the first row checks the CPU part.

## Checks

From the worktree, `CARGO_TARGET_DIR=.worktrees/target-am1`:
- fmt, workspace clippy, wasm32 clippy (site and edge, with and without `test-faults`, and ops), `cargo test
  --workspace`, Go, and the three design checks. See the commit message for each exit code.

## Not done here

- `graft-m1-load` is not deployed. Its first deploy is `bash tests/am1/load34/deploy.sh`, with the row grant, before
  the site day's row.
- No hosted row has run, so the CPU per operation, `slotWaits`, `lagMs` and N = 100 are untested on Cloudflare.
