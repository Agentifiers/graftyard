# LM-1 rerun on graft-m2b after `gate-cache` (2026-11-10)

This round reruns the 500-agent rows and LF-13 on graft-m2b from `origin/main` `1e25da8` (fanout, M4-WP1, then
`gate-cache` `04487ca`). The question is whether caching installation-token gate decisions removes the limiter that
`fanout.md` section 3 found. Branch `lm1-rerun`, cut from `1e25da8`. graft-m1 was not touched; B0-30 runs there.

## Summary

- **The gate cache works.** A 100-agent smoke at journey 9's shape landed 101 of 101 pushes and 4,869 of 4,869 API
  reads, with 0 `/suspect`. That scale failed only on the validate queue before `fanout`, and on uncached gate
  reads after it.
- **Every 500-agent row still fails.** The rows are B0-29, B0-31, B0-32, B0-33 and the L0-12 A1 replay; B0-34 was
  stopped early, and B0-35 was not run (see "Cost"). The 503s are still the gate's "the repository's access rows
  did not answer", but the cause has moved. **The repository group's members now run out of memory**: they peak at
  250–330 MiB against the 128 MB isolate limit, Workers Logs shows thousands of `exceededMemory` outcomes, and then
  come resets, `StepDown { CommitErrors }`, a `/suspect` storm and leader churn. During each election, the gate's
  `access_rows` read finds no leader and answers 503.
- **The likely source of the memory is the edge's whole-tier `{"rows": true}` reads.** Every REST and GraphQL read
  (`api::tier_rows`) does one, and so does every agent push (`agents::push_context`). Each dumps every row of both
  tiers, 0.5–1.5 MB, on the leader. This part is an inference with supporting evidence (section 3). It isn't built
  or tested as a fix.
- **LF-13 still fails both modes.** Install reached its fault in 2 of 3 runs: new polls were refused (48 and 47 of
  50 `lagging`), every held poll came back with its cursor and no events, and no event was lost. But only 630 and
  479 of 999 held polls returned within 1 s of the lift; the rest returned before it, and the slowest after it came
  at 1,003 and 1,004 ms. Gap formed no gap in 3 of 3 runs, because O was level with F each time.

## 1. Deployed

- **Site day** from `1e25da8` (`tests/fanout/run-siteday.sh`, `TAG=r`; log `tests/am1/raw/runs/fanout-r-siteday.log`):
  - Image `registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:172216ad0cbdf8779b0c20a698a0b5040a3de4295581408698ea3b157a8df3e9`.
    `containers/` hasn't changed since `e46da45`.
  - Final site versions: a `10e06185-b7d3-4a80-ba9c-7eba7f776534`, b `dea87a60-8631-4021-b0f2-b439e407f6a0`,
    c `579774d0-2a29-420b-b80f-2c94d759c989`. The first pass was `56b09c49` / `35f65d1a` / `507175cc`.
  - `--roll-shards` was stopped after 4 instances, because each catch-up was slow. Shards are derived caches, and
    each row below uses a fresh repository whose shards start on the new image. The stale shards were torn down
    later (`tests/fanout/teardown.py`).
- **Edge** `b94893bd-cced-4665-8066-fb047162a60d`, from `1e25da8`, with the same `FORMATS_ENABLED`
  (`32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2,32.8,32.12,32.14,18.3`) and
  `OPS=graft-m2b-ops`. Log: `tests/am1/raw/runs/fanout-r-edge.log`. Ops unchanged.
- This deploy carries `105c228` / `320b4a1` (a follower answers `/fanout` with `notLeader`), `dfa5410` (the feed
  ring restarts after a hole), and `04487ca` (the gate cache for installation tokens).

## 2. The rows

The rows used `tests/fanout/rows.sh` (`TEARDOWN=1`, a fresh repository per row, k = 4 through `/bench_seed` except
in B0-34) driving `tests/am1/rest/a1.mjs` at journey 9's 500 agents (B0-33 at 5,000). Driver rows are in
`tests/am1/raw/a1-<row>-<run>.jsonl` (gitignored, kept on the orb).

In the tables:
- "503 gate" means the edge's "the repository's access rows did not answer; retry".
- Memory is the max `memoryUsageBytes` per member (`durableObjectsPeriodicGroups`).
- `exceededMemory` counts are Workers Logs outcomes per site script in the row's window, from the exact
  `calculations` view (`tests/fanout/count.py`).

| Row | Run (repo) | Pass line | Measured | Verdict |
| --- | --- | --- | --- | --- |
| Smoke, 100 agents | `mwab1vd2`, 120 s | (no gate line; checks the cache) | Agent pushes **101 of 101 ok**. API reads 4,869 × 200. Merges 13 ok, 4 × 409. Commit p50 527 ms. Leader `/suspect` 7. Feed p99 2.5 s. Member memory peaks 32–74 MiB | ok |
| B0-29 | `mwabbn2s` (131202), 600 s, k 4 | "Commit p99 at most 600 ms, merge p99 at most 2 s, zero `Stale` on distinct refs, leader under 600 req/s with zero feed polls" | Agent pushes **274 of 2,500 ok**: 1,425 × 503 gate, 224 × 500, 567 timeouts, 4 `stale`. Commit p50/p99 1,063/1,505 ms (n 144). Merges 2 of 3 ok, p99 76 s. API reads 4,143 × 200, 9,576 × 503. Feed p99 7.4 s. Leader held at site 0 (term 15). `exceededMemory` 11,117 / 4,478 / 2,814 on sites a/b/c | **FAIL** |
| B0-32 | `mwabuknd` (131203), 180 s, k 4 | "Every push acknowledged within 5 s or answered in an HTTP 200 with `ng <ref> agent admission full, retry after <n>s` … zero leader changes, and zero `suspect` reports" | Burst **0 of 500 within 5 s**: 456 × 503 gate, 44 timeouts, none in the `ng` shape. Agent pushes 183 of 759 ok. Leader 0 → 2 (term 4). Leader inbound `/read` 40,180, `/suspect` 51,362 | **FAIL** |
| B0-34 | `mwacdzar` (131204), 600 s, home shard killed at 300 s (200), k from `fanout` | "k set in band from `NetworkLoad` … human push p99 at most 3 s beyond transfer, and clone TTFB p99 at most 1 s throughout" | **Stopped by hand at about 07:15Z**, on a false cost reading (section 4), so it has no summary row. From the edge's logs for 07:02–07:15Z: receive-pack 103 × 200 of 2,577 (1,740 × 503, 204 × 500, 530 no reply), upload-pack 1,034 × 200 of 7,257 (5,825 × 503). k stayed 1; 3 `/fanout` reports reached members | **invalid / FAIL** |
| B0-31 | `mwadn4ff` (131206), 600 s, leader moved to site 2 (apac) first, k 4 | "Commit p99 at most 1 s with buckets up" | Agent pushes **48 of 2,500 ok**: 1,741 × 503 gate, 478 timeouts, 196 refused, 37 × 500. Commit p50/p99 1,887/2,359 ms (n 93). Leader ended at site 2, term 28. The placement-failing half was not reached | **FAIL** |
| B0-33 | `mwae5ul0` (131207), 5,000 agents, 300 s, k 4 | "Human and merge-queue commit p99 at most 1 s, at least 140 agent commands/s admitted, the user token admitted as `Agent`, every refusal in the git `ng` or REST 429 `retry-after` shape, zero leader changes" | Agent pushes **101 of 12,484 ok** (10,633 × 503 gate, 1,692 timeouts, 52 × 500, 4 × 502). Side commands admitted in 300 s: taskEdit 164, comment 125, lease 102, standIn 173, leaseRenew 57, about 2.1/s. Human pushes 0 of 29 attempted. Fast user 41 × 200, 494 × 503. Leader 0 → 1 (term 13). Leader inbound `/read` 48,104, `/suspect` 63,988. Memory 331 / 262 / 250 MiB. `exceededMemory` 28 / 2,320 / 9,496 | **FAIL** |
| L0-12, A1 replay, 500 pollers | `mwaeino1` (131208), 600 s, 500 tasks and 500 Shared leases, a leader kill every 60 s, k 4 | "500 long-pollers see each event within FEED_BATCH plus 2 s (3 s) at p99 outside leader kills" | Feed: **0 events timed** at the pollers, from 146,705 polls (82,839 `gone`, 43,670 no status, 15,809 `notServing`, 2,994 `hold`, 1,393 `lagging`). The leader's ring held 1,401 events through 1,191. Kill pushes 1 of 10 ok (9 × 503 gate). Agent pushes 137 of 2,490 ok (2,094 × 503 gate). Lease renews 25 of 1,554 ok. Leader still at site 0 (term 21). Inbound `/read` 84,482 (f2) and 54,703 (f1); `/suspect` 92,796 / 87,783 / 56,352. Memory 283 / 280 / 265 MiB. `exceededMemory` 1,698 / 5,624 / 2,587 | **FAIL** |
| B0-35 | not run | "Test-merge vCPU within `TEST_MERGE_BUDGET`, Actions dispatch p99 at most 10 s, and zero computes from list fields" | Not run. It would start at $11.42, against the $12 row stop in `rows.sh`. The last round's B0-35 took 7.5 min of setup plus 600 s and failed at the same 503s | **not run** |

A diagnostic B0-29 with API reads off (`mwad1xjk`, 131205, 300 s) ran between B0-34 and B0-31:
- 600 of 1,249 agent pushes ok, merges 18 of 21 ok, and 5,234 feed events (p99 6.6 s).
- The first 2 minutes were clean, with 0 `/suspect`.
- Then the site-a and site-b members hit `exceededMemory` (1,435 and 1,361 outcomes), and the `/suspect` storm
  followed (3,031 on the leader). By then the repository held about 500 `PathSet` and 660 ref rows.

## 3. The new limiter: member memory

**Measured:**
- Repository members peaked at **250–331 MiB in every 500-agent row**, against 32–74 MiB at 100 agents. A Durable
  Object isolate's limit is 128 MB.
- Workers Logs records `exceededMemory` outcomes in every row. They are followed by "Durable Object's isolate
  exceeded its memory limit and was reset" and `StepDown { CommitErrors }`.
- `graftyard_lookup` then reports the missed members with `/suspect` to the other voters, at 23–34 k a minute, and
  the leader moves. A read during the election answers `not leader` or `no answer`. L0-12's kill pushes show the
  exact text: "access rows did not answer; retry (repo:N: repo:N#N#fN: not leader)".
- Leader `/read` load was about 7–11 k a minute per member. The gate cache removed the gate's per-push read for
  `ghs_` tokens (the smoke's 0 `/suspect`), but other `/read`s remain:
  - Every REST and GraphQL read goes through `api::tier_rows`, `{"q":{"rows":true}}`, which returns both tiers'
    rows. Measured on 131205: 1.46 MB, 1.5 s.
  - Every agent push runs `agents::push_context`, which calls the same `tier_rows` and `install_authz`, to find the
    repository's `AgentPolicy` and tasks.
- In a direct test on one leader, 60 parallel `{rows:true}` reads gave 50 of 60 failures (exceptions, wall about
  2.2 s). 60 parallel `{rows:{access:true}}` reads all succeeded.

**Inferred:**
- Journey 9 offers about 200 API reads/s plus 4.17 agent pushes/s. That is on the order of 200 whole-tier dumps of
  about 1 MB each per second, on one leader's 128 MB isolate.
- With API reads off, the dumps per push alone (`push_context`) grow with the repository and reach the limit after
  about 2 minutes. That matches the diagnostic run.
- I didn't isolate how much of the memory comes from `tier_rows` against commits' own working sets.

**Proposed fix (not built).** It needs site and edge code plus a site day, so it is larger than the "small, clearly
scoped" fix this task allows, and this round had no budget left for it:
1. `push_context` reads only what it uses: the `AgentPolicy` row and the pushing App's task rows, through a
   targeted `rows` query like `{"rows":{"access":true}}` and `{"rows":{"merge":id}}`, instead of `TierRows::dump`.
2. REST and GraphQL reads ask for the rows their resolver needs (one PR, one issue, a page of a list), not the
   whole tier. The alternative is a short per-repository cache of tier rows at the edge, at `READ_CACHE`.
3. Then rerun the smoke at 250 and 500 agents, and watch member memory and `exceededMemory` before the rows.

## 4. LF-13 (both modes, 3 runs each, `HELD=999`)

`tests/fanout/lf13.sh` drives `tests/am1/rest/lf13.mjs`, with `HELD=999` so that new polls aren't masked by `full`
at `FEED_HELD_MAX`. Logs: `tests/am1/raw/runs/flf13-<mode>-<n>.log` and `tests/am1/raw/lf13-<mode>.jsonl`.

Pass line (`build/BUILD-PLAN.md`, AM1-WP4): "Every held poll returns within 1 s with its cursor and no events, new
polls are refused until it catches up, and no event is lost".

| Run | Fault reached? | Stopped | Before lift | Within 1 s | After-lift p50/max ms | `lagging` of 50 | Events F / ref / missing on F |
| --- | --- | --- | --- | --- | --- | --- | --- |
| gap 1 | no gap: O level with F (127/127); fault 31.7 s | 999 | 999 | 0 | — | 0 | 2 / 1 / 0 |
| gap 2 | no gap (132/132) | 0 | 0 | 0 | 2,908 / 3,891 | 0 | 2 / 1 / 0 |
| gap 3 | no gap (138/138) | 0 | 0 | 0 | 4,923 / 5,915 | 0 | 2 / 2 / 0 |
| install 1 | no: not truncated past F in 18 s | 0 | 999 | 0 | — | 0 | 2 / 1 / 0 |
| install 2 | **yes**, truncated to 1,224, F at 1,222, at 22.0 s | 193 | 358 | 630 | 673 / 1,003 | **48** | F `gone` (served 48.4 s after the lift) / 0 / 0 |
| install 3 | **yes**, 1,340 past F at 1,338, 19.7 s | 0 | 518 | 479 | 446 / 1,004 | **47** | F `gone` (50.4 s) / 0 / 0 |

**Install: FAIL, narrowly.**
- In both runs that reached the fault, every one of the 999 held polls came back with its cursor and no events.
  New polls were refused (47–48 `lagging`), and no event was lost (F answers `gone` after the install, as the
  ring rule says).
- No held poll returned later than 1,004 ms after the lift. But the slowest were 3–4 ms over 1 s, and 358–518
  returned before the lift, most at their own 25 s wait (only 193 and 0 stopped by F). So F still doesn't stop its
  held polls when an install begins, in every run.

**Gap: FAIL.**
- No run left a gap: each time F stopped, O was level with it, so the transfer elected a leader whose first round
  matched F.
- Held polls returned 2.9–5.9 s after the lift, with 0 `lagging` and no event lost.
- This is the case `crates/raft/tests/lf13.rs` calls `rounds_lost_to_the_same_leader_leave_no_gap`. With the site
  fault's 1 s resolution (`SITE_FAULT_TTL_MS`), a refused round can't be placed reliably hosted.
- The task asked for "a fault that really leaves a gap, held polls the driver's own writes can't wake". The second
  half holds. The first was reached in only 1 of 9 hosted gap runs so far (fanout r3 gap 2), and not this round.
  A test-faults hook that makes the leader skip F's next `AppendEntries` would place the gap exactly. I didn't
  build it this round.

## 5. Cost

`tests/fanout/cost.sh` with `T0_FILE=tests/fanout/raw/siteday-r.t0` (06:08Z) now calls `tests/fanout/m2bcost.py`.
It counts graft-m2b only: Workers requests and CPU of the graft-m2b-* scripts; Durable Object requests, rows written
and active time of the graft-m2b-* namespaces; and the graft-m2b-site-*-gitshard containers' measured vCPU, memory
and disk (`containersUsageAdaptiveGroups`). Everything is at `research/prices.json` overage prices.

- **Total for this round: $12.41** at 08:46Z, of which **containers were $9.22**. That covers the site day, the
  edge, the smoke, B0-29, B0-32, the stopped B0-34, the diagnostic B0-29, B0-31, B0-33, L0-12 and LF-13 (LF-13
  alone was $0.82). It is under the $15 cap.
- The old formula, account-wide DO rows and active time, read $17.85 at 07:15Z. It was counting graft-m1's B0-30
  writes, about 12 M rows. That reading is why B0-34 was stopped. The graft-m2b-only figure for the same window was
  $5.73.
- Containers dominate. A `durable_object` shard container bills its memory and disk while it runs. After the rounds,
  `teardown.py 131170 131208` stopped 23 shards still running (23 × 200). The pool shards `git#0#pool:1-7` are left
  running and keep billing until they are torn down or sleep.

## 6. What LM-1 still lacks, besides B0-30

1. **Repository members that hold journey 9's reads in 128 MB** (section 3): targeted row reads for `push_context`
   and the REST and GraphQL resolvers, or an edge cache of tier rows. Until then B0-29, B0-31 to B0-35 and L0-12's
   A1 replay fail at the gate's 503 during member resets.
2. **`fanout` stepping k in band, shown hosted.** B0-34 needs a full rerun. The `/fanout` reports now answer `ok`
   (16 of 17 in B0-29), so `105c228` works, but k wasn't seen to step this round.
3. **B0-35**, run once (1) is in. Its setup (500 PRs) worked last round. Test-merge vCPU and list-field computes
   are still not instrumented.
4. **LF-13**:
   - for gap, a fault that places the gap exactly;
   - for install, F stopping every held poll when the install begins, so all return within 1 s.

## 7. Design contradictions (quoted; no design file edited)

- `design/DESIGN.md`, journey 9, step 1: "its leader about 305 req/s". This counts requests, not their size. Each
  edge REST/GraphQL read and each agent push's `push_context` is a whole-tier `{"rows": true}` dump of 0.5–1.5 MB
  on the leader. Measured: 7–11 k leader `/read`s a minute per member, and members at 250–331 MiB against a 128 MB
  isolate.
- The contradictions in `fanout.md` section 6 still stand: in-band `NetworkLoad` only while validating, the
  rendezvous pick not limited to caught-up shards, k = 1 at journey 9 not shown, and LF-13's "within 1 s" of what.

## 8. Commits on `lm1-rerun` (from `1e25da8`; not pushed, not merged)

- `4cdc1b6`: `containers.py` (measured container cost), `teardown.py`, `rows.sh` `TEARDOWN`, `run-siteday.sh`
  `TAG`, `lf13.mjs` `HELD`.
- `21740bb`: `m2bcost.py` (graft-m2b-only cost; `cost.sh` with `T0_FILE` uses it), and `count.py`, `byobject.py`
  and `edgekinds.py` (exact Workers Logs counts).
- This results commit.
