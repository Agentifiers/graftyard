# `merge-budget-2`: A28, A29 and A30 closed (2026-11-12)

Branch `merge-budget-2`, from `origin/main` `5e6fbe2`, with `lf4-rest-2` and `lf4-rest-3` merged in. Hosted on the test install `graft-m3c` (`tests/am1/results/install-m3c.md`). Nothing ran on `graft-m1-*` or `graft-m2b-*`.

## Summary

- **A28, overrun.** The cause is confirmed. Round 5's first attempts timed out and their re-sends ran side by side, so up to 5 of the network's merges were in flight at once. Each was admitted at its 50 ms estimate before those ahead of it were charged. Fix: a network's merges now run one at a time, each admitted only after the one before was charged. Each is charged its own CPU (`cpuMs` from the container), so the bucket goes below zero by at most one merge.
- **A28, size.** The design's sizing stands: journey 9 is a `standard-4`, with a 1 vCPU budget. The site now sizes its shard and budget from `SHARD_INSTANCE`, which is unset on test installs and so `standard-2`. It refuses an unknown instance name rather than pricing it as `standard-4`.
- **A30.** A single-PR GraphQL read with no current merge has the edge ask the shard for a GET merge after it replies (`/demand?cause=get`). The shard coalesces, budgets, computes and commits it. `crate::prsync::demand` is the hook M4's `ActionsTrigger` calls.
- **A29.** List reads that render `mergeable` with no current merge are counted on the shard (`/ask?cause=list`) and never compute.
- **Hosted** (`361a96e`, site day 06:31Z to 06:40Z, two B0-35-shaped runs on App tokens, 50 PRs each):

  | Run | Head pushes | vCPU used of 0.25 | Lowest bucket | GET merges computed | List asks counted, computed |
  | --- | --- | --- | --- | --- | --- |
  | 1 | every 30 s | 0.129 | 0.0 ms | 254 of 620 asks | 1,547, 0 |
  | 2 | every 5 s | 0.085 | -12.1 ms, against a largest merge of 33.7 ms | 219 of 1,039 asks | 2,365, 0 |

  Neither run overran.
- **Plants.** 35 of 35 caught (`tests/budget/plant.py`).
- **Cost.** $0.30 for 06:30Z to 07:05Z, $0.02 of it containers.

## A28: the cause

Round 5's B0-35 spent 0.274 vCPU against 0.25 on `graft-m2b-site-a`, repo 131260 (`tests/am1/results/lm1-memory.md`, section 12). Workers Logs from that run show:

- First attempts timed out ("no answer in 25 s"). Their re-sends through the queue ran concurrently: up to 5 consumer handlers overlapped, each with a merge step of 60 to 240 s and 120 to 507 asks.
- Each merge was admitted at the 50 ms estimate while earlier merges were still uncharged.
- The worst window spent 31,928 ms in 107.8 s against an allowance of 26,950 ms, an excess of 4,976 ms. It ran from 21:44:04 (`spentMs` 147,335) to 21:45:52 (179,263).
- Charges were wall time: 164,135 ms over 707 merges is about 232 ms a merge. A test merge costs 24.7 ms of CPU by cgroup in a one-vCPU container bench (200 sequential merges, wall p50 24.1 ms). There, the new `cpuMs` reported 20.5 ms, the git processes alone without the server's share, so it never over-charges.

The old admission let a push run while the bucket was above minus one estimate. With n merges in flight, all admitted before any was charged, the spend could pass the budget by n merges, not one.

## What changed

1. **Design first.** The agent block in `design/DESIGN.md` and `design/AGENT-LAYER.md`, kept byte-identical (`scripts/check_design.py`):
   - Merges run on a head push, on Actions dispatch (`demand`, which `ActionsTrigger` calls), and on a single-PR GraphQL read by a writer or CI token with no current merge.
   - The shard counts each PR a list read rendered without one.
   - The shard runs at the tier's instance size.
   - A network's merges run one at a time, each admitted only once the one before was charged its CPU, so the spend passes the budget by at most one merge.
   - Round 5's cause is stated, and journey 9's 0.21 vCPU is "of a `standard-4`'s 1".

   `decisions/DECISIONS.md` adds "A28 closes", "A30 closes" and "A29 closes". `decisions/OPEN-QUESTIONS.md` drops A28 to A30, and `build/BUILD-PLAN.md` (AM1-WP1, the LM-1 list) follows.
2. **`graftyard_git::prsync`** (`crates/git/src/prsync.rs`):
   - `MergeBudget` splits into:
     - `ask`: counts list and GET asks and coalesces GETs per PR for `GET_COALESCE_MS`.
     - `would`: pushes and dispatch need a positive balance; GETs keep `GET_RESERVE`.
     - `decide`: a shed GET frees its slot.
     - `charge(cause, est, cpu, wall)`.
   - `admit` is `ask && decide`.
   - `BudgetStats` adds `wallMs`, `getAsked`, `getComputes`, `lowMs` and `maxMs`, and reads older JSON with defaults.
   - `Budgets` has a turn per network. `asked` sheds before the catch-up, decides again in its turn, then charges the merge's CPU, or its wall time when the container sent none. `budgeted` is `ask` then `asked`.
   - `on_demand` builds a GET's or dispatch's command: a `PrSync` with one merge, no head moves and no cursor, keyed by its content.
   - `SHARD_VCPU` is gone. `instance_vcpu` returns `None` for an unknown name.
3. **The container** (`containers/git/server/main.go`): the test merge's answer carries `cpuMs`, the user plus system time of its own git processes.
4. **The site** (`workers/site`):
   - `SHARD_INSTANCE` names the shard's instance type and sizes `Budgets`.
   - `POST /ask?cause=list&count=k` counts k list asks and starts no container.
   - `POST /demand?cause=get|dispatch&n&base&head` asks the budget, merges and commits.
   - `/merge` goes through `asked`.
   - `crate::prsync::demand(env, repo, n, cause)` reads the PR fresh and sends `/demand`; M4's `ActionsTrigger` calls it.
5. **The API and edge**:
   - `graftyard_api::graphql::query_asks` renders a query and returns the `MergeAsks` its `mergeable` and `mergeStateStatus` fields lacked: single PRs with their (base tip, head), and listed PRs.
   - After the reply, in `wait_until`, the edge's `ask_merges` (`workers/edge/src/api.rs`) sends `/demand?cause=get` per single PR for a writer or an App token, and one `/ask?cause=list` for the rest. The reply bytes do not change.
6. **Cost** (`scripts/cost_agents.py`): the budget is one shard's, not k shards'. `fanout` sizes k by load alone, and `merge_budget_fits` checks head-push merges against one `standard-4`'s budget. The printed figures are unchanged (k = 1 at journey 9), so `research/costs.md` is unchanged.
7. **The driver** (`tests/am1/rest/a1.mjs`):
   - B0-35 reports `getAsked`, `getComputes`, `listAsked`, `cpuToWall` and `overrun`.
   - It samples the shard every 15 s, because the shard object can restart after the load (run 1 below).
   - Scripts: `tests/am1/mb2/siteday.sh` and `hosted.sh`.

## Tests and plants

New tests:

| Test | What it fails on |
| --- | --- |
| `crates/git/tests/agent_load.rs`, `concurrent_merges_overrun_the_budget_by_at_most_one_merge_and_are_charged_their_cpu` | Admission before the merges ahead are charged. 10 merges of 300 ms CPU over 900 ms wall in two bursts of 5, on a 0.25 vCPU budget. Expected: 4 computed, 6 shed, low -275 ms, 1,200 ms charged. Also: a bucket 10 ms short admits no push until it refills. |
| `gets_coalesce_per_pr_a_shed_get_frees_its_slot_and_list_asks_are_counted` | GET coalescing, the reserve, a shed GET's slot, and list counts. |
| `crates/git/tests/prsync.rs`, `an_on_demand_merge_records_the_current_head_and_moves_no_cursor` | A GET merge that moves the cursor, commits twice, records an older head, or commits past the command bound. |
| `crates/api/tests/surfaces.rs`, `a_read_asks_for_the_merge_its_mergeable_lacks_and_a_list_only_counts` | A list that asks for computes, an ask for a PR whose merge is current, an ask with the wrong base, and records leaking outside `query_asks`. |
| `a1.mjs --selftest` | The new window fields, an older shard's JSON, the overrun line, and the sample kept when the shard restarted after the load. |

Existing tests updated: `the_shard_budgets_each_network_once_for_all_its_callers`, `a_head_push_past_the_test_merge_budget_leaves_mergeable_null`, and B0-35 in memory, which now checks `getAsked`, `getComputes`, and that a coalesced GET is not counted as shed.

Planted bugs (`python3 tests/budget/plant.py`), 35 of 35 caught. The first full pass caught 32 of 34. It missed "pushes admitted down to minus the estimate", which needed a test of the admission threshold itself, and "`lowMs` not recorded at admission", whose plant had targeted a redundant line, now deleted. Both are caught now; the shed-cause plant was added after that pass and caught on its own.

| # | Plant |
| --- | --- |
| 0 to 15 | merge-budget's 16, rewritten for the new code: never charged, estimate kept, nothing spent, per caller, one budget for all networks, never shed, shed recorded as a merge, a list ask passed on, list asks not counted, list computes not counted, sheds not counted, the catch-up charged, shard size ignored, a shed merge still catching up, two `/stats` plants |
| 16 to 19 | Driver: a restarted shard subtracted; the overrun line ignored; a shard restarted after the load losing the run's sample; vCPU counting one site |
| 20 to 24 | A28: no turn; one decision before the catch-up and turn; pushes admitted down to minus the estimate; wall time charged; `lowMs` never recorded |
| 25 to 31 | A30: GETs never coalesce; a shed GET keeps its slot; GET asks not counted; GET computes not counted; a GET ignoring the reserve; an on-demand merge moving the cursor; an on-demand merge past the bound committed |
| 32 to 34 | A29 and A30 on the edge: a list asking for computes; a current PR still asking; the ask naming the recorded base, not the tip |

Checks on the final tree, all passing:
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -D warnings`
- wasm32 clippy for site and edge, with and without `test-faults`, and for ops (ops has no `test-faults` feature)
- `cargo test --workspace` (74 suites)
- `go test ./...` in `containers/git/server`
- `scripts/check_design.py`, `check_names.py`, `check_links.py`
- `a1.mjs --selftest`

No wire enum changed. `MergeAnswer` and the new `/demand` and `/ask` answers are JSON, and the GET's `PrSync` is the existing `RepoCmd::Sync`.

## Hosted on `graft-m3c`

### Site day

- `361a96e` (main `0a1d60c` plus `lf4-rest-2`, `lf4-rest-3` and this branch), test-faults, `--anchor ~/.graft-m3c/install.toml`.
- New shard image `graft-m1-site-gitshard-git:mb2` (`sha256:5f4f7f93…`, `tests/am1/mb2/siteday.sh`), so `cpuMs` is reported.
- Site day 06:31Z to 06:40Z, `site exit 0`. Site a is now version `50732405`.
- `graft-m3c-edge` is `14cc4626`, with `OPS = graft-m3c-ops` and `lf4-rest-3`'s `FORMATS_ENABLED`, including 32.18.
- `SHARD_INSTANCE` unset, so shards are `standard-2` with a 0.25 vCPU budget.
- The run was coordinated on graft-m3c after lf4-rest-3's window and before m0-carried-3 and hot-shards.

### Runs

```
bash tests/am1/mb2/hosted.sh mb2-b035 mb2r1 50 30 300     # repo:65549, run 1
bash tests/am1/mb2/hosted.sh mb2-b035h mb2r2 50 5 180     # repo:65550, run 2
```

Both runs:
- Used fresh repositories, 50 open PRs (one per agent), and the agents pushing their own PR's head.
- Ran under App tokens: the m3-ci installation token for pushes and `mergeable` polls, flagged through the agent-apps route now that m3-admin is a site admin.
- Sent `mergeable` polls as single-PR GraphQL every 5 s per PR, list reads with `mergeable` under the admin's PAT, and base moves by `PUT pulls/{n}/merge`.

Run 1 pushes each head every 30 s, 4 times journey 9's per-PR rate. Run 2 pushes every 5 s, as merge-budget's run did.

| Measure | Run 1 (06:44:55Z, 300 s) | Run 2 (06:57:32Z, 180 s) |
| --- | --- | --- |
| Head pushes | 500 ok (p50 3.4 s) | 1,284 ok (p50 7.2 s) |
| Test merges computed | 760 | 688 |
| Shed | 50 | 135 |
| Test-merge CPU | 38.9 vCPU-s in 302.6 s = **0.129 vCPU** of 0.25 | 15.2 vCPU-s in 180 s = **0.085 vCPU** of 0.25 |
| Lowest bucket (`lowMs`) and largest merge (`maxMs`) | **0.0 ms** and 102.6 ms | **-12.1 ms** and 33.7 ms |
| Overrun beyond one merge | No | No |
| Merge CPU / wall | 0.52 | 0.23 |
| GET asks, GET merges computed | 620, **254** | 1,039, **219** |
| `/demand` requests | 637, all 200 | 1,039, all 200 |
| List asks counted, list computes | **1,547, 0** | **2,365, 0** |
| `mergeable` polls | 873 `MERGEABLE`, 637 `UNKNOWN`, 1,595 HTTP 503 | 47 `MERGEABLE`, 1,039 `UNKNOWN`, 736 HTTP 503 |
| Leader | site 0, term 2 | site 0, term 2 |

Run 1's counts come from the shard's own `x-graft-budget` on each merge reply in Workers Logs, the last line before and after the window. The driver's after-window read found `testMerges: null` on every site: the shard object restarted between the load's end and the read (06:50:02 to 06:51:21Z), and its counts are in memory. Run 2's come from the driver, which read before the restart (a later poll at 07:01:28Z again found null). The driver now samples every 15 s and keeps the latest.

What the runs show:

- **No overrun.**
  - Run 1's bucket never went below zero.
  - Run 2's went 12 ms below at its lowest, against a largest single merge of 34 ms.
  - Both spent well under 0.25 vCPU on average. Merges on these small repositories cost 20 to 100 ms of CPU, and the bucket holds one second of budget, so bursts of a batch's merges are shed while the average stays low.
  - These runs do not recreate round 5's load (500 agents, queue re-sends in flight). The concurrent case is shown by the in-memory test above and by the turn, which those re-sends now wait on.
- **GET merges admitted.**
  - 254 and 219 merges were computed for single-PR GETs, out of 620 and 1,039 asks; the rest coalesced per PR or were shed past the reserve.
  - With heads moving every 30 s, 873 of 1,510 answered polls were `MERGEABLE`. merge-budget's run had 60 of 3,012, but at heads every 5 s.
  - At heads every 5 s, most GET merges are stale before the next poll (47 `MERGEABLE`).
  - A poll that asks answers `UNKNOWN` itself, as GHES 3.22 does while it computes; the next one reads the merge.
- **List count working.** 1,547 and 2,365 list asks were counted, and 0 computes.
- **Merge wall time is 1.9 to 4.4 times its CPU.** Wall time includes waiting for the container's vCPUs behind validates and catch-ups, so charging wall time would have over-charged by that much.

Not caused by this change, recorded for the owners:
- **HTTP 503s on polls and list reads** ("the repository's tier rows did not answer; retry"). The members shed reads, the read path LM-1 is working on (F65). The edge's asks run in `wait_until` after the reply, so they cannot change a reply's status [inference].
- **"Subrequest depth limit exceeded" on `POST /merge`.** 16 in run 1's first 3 s and 4 in run 2, on the cold shard's first catch-up. merge-budget's run on 2026-11-11 logged 20 at its start too.

Shards torn down after the runs (`INSTALL=graft-m3c python3 tests/fanout/teardown.py --all-nets --pools`): `git#0#net:65550`. `git#0#net:65549` had already stopped. A second pass found none running.

### What this does not show

- B0-35 as written: 500 PRs and 500 pollers. The 500-agent bar (F65) still fails on the read path.
- Actions dispatch p99. No test install has Actions; the hook exists and M4 calls it.
- A `standard-4` shard's 1 vCPU budget. `SHARD_INSTANCE` was not set on `graft-m3c`.

## Design changes and contradictions

1. **The agent block now says what admission must do** (above): merges one at a time, each charged its CPU; the overrun bound of one merge; the tier's instance size; `demand`; and GET and list asks from the edge.
2. **Contradiction, settled:** the code's `SHARD_VCPU` = 4 against the site's fixed `standard-2`. Neither was the design's error. The design sizes shards by tier, so the site now takes `SHARD_INSTANCE`, and the code no longer guesses `standard-4`.
3. **Contradiction, fixed:** `scripts/cost_agents.py` sized `TEST_MERGE_BUDGET` as "the k shards' vCPU" after A27 had fixed the design. The figures are unchanged.
4. **Left as is:** REST `GET pulls/{n}` (lf4-rest's `rest::get_pull`) always answers `"mergeable": null` and asks nothing, because its renderer is the webhook's. The decision's flip line names REST pollers as the reason to change it.
5. **New finding:** the shard object restarted within about 90 s of its last request, so B0-35's counts must be read during the run, not after. The design keeps the budget in memory ("a restart starts it full" [inference]), which this confirms.

## Cost

$0.30 for the hosted window, 06:30Z to 07:05Z (`INSTALL=graft-m3c python3 tests/fanout/m2bcost.py 2026-11-12T06:30:00Z 2026-11-12T07:05:00Z`): 12,688 Workers requests, 211,197 DO requests, and 214 container CPU-s, $0.02 of it containers. The image push and local work cost nothing on Cloudflare.
