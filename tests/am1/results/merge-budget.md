# `merge-budget`: `TEST_MERGE_BUDGET` enforced, and B0-35's instruments (2026-11-11)

AM1-WP1's follow-up, "The test-merge budget and B0-35's instruments" (`build/BUILD-PLAN.md`), and open question A27 (`decisions/OPEN-QUESTIONS.md`). Branch `merge-budget`, from `origin/main` `6fe479e`. Hosted on the test install `graft-m3c` (`tests/am1/results/install-m3c.md`). Nothing ran on `graft-m1-*` or `graft-m2b-*`.

## Summary

- **The budget is spent, shed, and reported.** The git shard object that runs a network's test merges keeps that network's `MergeBudget` in its memory. It admits each merge by cause, charges the merge's measured time, answers `{"shed": true}` past the budget, and reports its counts. Before this change, `workers/site/src/prsync.rs` kept a budget per consumer isolate and never charged it.
- **Hosted on `graft-m3c`** (`4b95a3e`; site day at 16:21Z to 16:31Z; a B0-35-shaped run from 16:33:54Z, 300 s): one repository with 50 open PRs and 50 head pushes every 5 s (10/s, 2.4 times journey 9's per-network rate). The base moved by 27 merges, plus 130 human pushes to other branches. The shard computed 190 test merges and shed 694. It spent 55.3 vCPU-s in 300 s, which is 0.1845 vCPU against a budget of 0.25 (`standard-2`, 1 vCPU). It computed 0 merges for list fields. 3,012 `mergeable` polls answered `MERGEABLE` 60 times and `UNKNOWN` (null) 2,952 times.
- **Plants:** 18 of 18 caught (`tests/budget/plant.py`).
- **Cost:** $0.27 for the hosted window (16:20Z to 16:45Z, `INSTALL=graft-m3c python3 tests/fanout/m2bcost.py`), $0.09 of it containers.

## What changed

1. **`graftyard_git::prsync`** (`crates/git/src/prsync.rs`):
   - `MergeBudget::new(vcpu, now)` refills at TEST_MERGE_BUDGET of one shard's vCPU (`instance_vcpu`).
   - `charge(cause, est, real)` corrects the bucket by the measured cost and counts it.
   - `admit` counts each shed merge, and each ask from a list field, which never computes.
   - `BudgetStats` carries `spentMs`, `computed`, `shed`, `listAsked`, `listComputes`, `since`, and `vcpu`.
   - `Budgets` holds one budget per `NetworkId` in a shard object.
   - `budgeted` admits the merge, then runs the catch-up (not charged) and the merge, and charges the merge's own time.
   - `MergeAnswer::Shed` is the shard's answer past the budget, and `take` sends it to `shed`, so `commands` deletes the old merge ref and `mergeable` stays null.
   - `MergeAnswer` is a JSON answer, not a stored or wire enum, so `wire_tags.golden` is unchanged.
2. **The shard** (`workers/site/src/shard.rs`): `GitShard` holds `Budgets` for its instance type (`standard-2`, from one `INSTANCE` constant). `POST /merge` without `method` takes `cause` (`push` by default) and `n`, and runs through `budgeted`. Its reply carries `x-graft-budget`, the network's `BudgetStats`. `/status?repo=` adds `testMerges`.
3. **The consumer** (`workers/site/src/prsync.rs`): the isolate `thread_local` `BUDGETS` is gone. Each head-push merge asks the shard with `cause=push`, and the shard's answer decides. `one` returns `asked`, `shed`, and `budget`. A first attempt's answer goes into the leader's `/stats` as `testMerges: { shard, asked, shed }` (`workers/site/src/host.rs`, `member.rs`). `shard` is cumulative. `asked` and `shed` reset with `/stats?reset`.
4. **The A1 driver** (`tests/am1/rest/a1.mjs`):
   - B0-35 opens one PR per agent (500 at the row's size), and each of those agents pushes its PR's head branch, so every push asks for a test merge. Before, agents pushed their own `claude/a1-*` refs, which no PR named.
   - It adds `mergeable` to its list reads.
   - It reads every site's shard budget before and after the window through `/_bed/shard_status`.
   - It writes `testMerges`: per site, the window's counts, or the whole count when the shard object restarted. It also writes `vcpuUsed`, `budgetVcpu`, `withinBudget`, `listComputes`, the leader's `/stats` field, and the `mergeable` poll answers.
   - `--push-every` and `--as-admin` scale the row and run it under the admin's PAT, for installs that cannot mint App tokens. `--selftest` checks the window arithmetic.
5. **`tests/fanout/teardown.py`** takes `INSTALL`, as `m2bcost.py` does, so it tears down `graft-m3c` shards.

## Design change

`design/DESIGN.md` and `design/AGENT-LAYER.md`, Shape, agent block (kept byte-identical):

> was: "`MergeBudget`, per network in shard memory, refills at TEST_MERGE_BUDGET of the k shards' vCPU: pushes and dispatch may run one merge into deficit, and GETs leave half a second's budget."
>
> now: "`MergeBudget`, per network in the memory of the shard that runs its merges (the leader site's first), refills at TEST_MERGE_BUDGET of its vCPU and is charged each merge's measured time; pushes and dispatch may run one merge into deficit, GETs leave half a second's budget."

Why: test merges run on one shard, the network's first shard on the leader's site (`workers/site/src/prsync.rs`, `merge`). They are never spread over the k shards, so a budget of k shards' vCPU would let one shard spend k times its share. The old text also did not say what the budget is charged with.

### Contradictions found

1. DESIGN, Shape: "`MergeBudget`, per network in shard memory, refills at TEST_MERGE_BUDGET of the k shards' vCPU". Only one shard of the k ever runs a network's test merges, so the k shards' vCPU is not what they spend. Fixed above.
2. `crates/git/src/prsync.rs` priced the budget at `SHARD_VCPU` = 4 ("A shard's vCPU at the tiers' instance size (`standard-4`)"), but the site script runs shards as `standard-2`, 1 vCPU (`workers/site/src/shard.rs`, `ShardHost { instance: "standard-2" }`). That allowed 1 vCPU of test merges, 100% of the shard. The budget now follows the instance type: 0.25 vCPU on `standard-2`. DESIGN's sizing still says "k = 1 `standard-4` shard per warm site" at journey 9. That claim is the tier's instance size, not what the site script deploys today, and it is not changed here. Journey 9's head-push merges are sized at 0.21 vCPU, which is within 0.25 either way.
3. DESIGN says list fields never compute, and that holds (0 list computes hosted). But the edge does not tell the shard about list fields at all: GraphQL's `mergeable` reads the recorded test merge (`crates/api/src/gql.rs`), and no read path calls the shard. So `listAsked` stays 0 hosted, and B0-35's "zero computes from list fields" is shown by `listComputes` = 0 together with that structural fact, not by a counted ask.
4. DESIGN runs test merges "on a head push, on Actions dispatch ..., and on a single-PR GET by a writer or CI token". Only the head-push cause is wired: the shard takes `cause=dispatch|get|list`, but nothing sends them yet. Actions dispatch is not on any test install, which B0-35's dispatch half already notes. A single-PR GET compute is not built on the edge. So past the budget, a polled PR stays null until its next head push.

## Tests and plants

New or changed tests:
- `crates/git/tests/prsync.rs`:
  - `a_head_push_past_the_test_merge_budget_leaves_mergeable_null` now goes through the shard's `budgeted` and the answer's JSON.
  - On `standard-2`, one 10 s merge empties the bucket, so the next push is shed although its 50 ms estimate would pass, and `mergeable` is null.
  - It checks the counts (1 computed at 10,000 ms, 1 shed).
- `crates/git/tests/agent_load.rs`:
  - `the_shard_budgets_each_network_once_for_all_its_callers`: two callers of one network, against one network alone, on one shard object.
    - Network 1's callers are held to its 0.25 vCPU together: 102 computed, 138 shed, 15.3 s spent in 60 s.
    - Network 2 is never shed: 60 of 60.
    - Only the merge's time is charged, not the catch-up's.
    - A shed merge runs no catch-up.
    - List asks are counted and never compute.
  - `a_compute_charged_to_a_list_field_is_counted`.
  - B0-35's in-memory test now also checks that the counts equal what the run did.
- `workers/site/src/host.rs`, `first_attempts_add_their_test_merges_to_the_leaders_stats`.
- `tests/am1/rest/a1.mjs --selftest` checks the window per site: a delta, a restarted shard, and a shard with no merges.

Planted bugs (`python3 tests/budget/plant.py`), 18 of 18 caught:

| # | Plant | Caught by |
| --- | --- | --- |
| 0 | Never charged: `budgeted` charges nothing | `the_shard_budgets_each_network_once_for_all_its_callers` |
| 1 | `charge` keeps the estimate, not the measured time | `a_head_push_past_the_test_merge_budget_leaves_mergeable_null` |
| 2 | Nothing counted as spent | `a_compute_charged_to_a_list_field_is_counted`, the per-network test |
| 3 | Per caller, not per network: a fresh budget each call | the per-network test |
| 4 | One budget for every network in the shard | the per-network test |
| 5 | Never shed | the per-network test |
| 6 | A shed answer recorded as a merge | `a_head_push_past_..._null` |
| 7 | A list field computes | the per-network test, B0-35 in memory |
| 8 | List asks counted as shed | the per-network test, B0-35 in memory |
| 9 | A list-field compute not counted | `a_compute_charged_to_a_list_field_is_counted` |
| 10 | Shed merges not counted | the per-network test, B0-35 in memory |
| 11 | The catch-up charged with the merge | the per-network test |
| 12 | The budget ignores the shard size (always `standard-4`) | `a_head_push_past_..._null` |
| 13 | A shed merge still runs the catch-up | the per-network test (missed in the first pass; the test now counts catch-ups) |
| 14 | `/stats`: a first attempt with no merge wipes the shard counts | `first_attempts_add_...` |
| 15 | `/stats`: asks overwritten, not added | `first_attempts_add_...` |
| 16 | Driver: a restarted shard subtracted from its old counts | `a1.mjs --selftest` |
| 17 | Driver: vCPU over the window counts one site only | `a1.mjs --selftest` |

Checks on the final tree: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -D warnings`, wasm32 clippy for site and edge with and without `test-faults`, and for ops, `cargo test --workspace`, `go test ./...` in `containers/git/server`, and `scripts/check_design.py`, `check_names.py`, and `check_links.py`, all pass. `crates/raft/src` did not change, so the long sweep and Porcupine were not run. No wire enum changed.

## Hosted on `graft-m3c`

### Site day

`4b95a3e`, test-faults, `--anchor ~/.graft-m3c/install.toml`, and the `graft-m1` shard image `@sha256:172216ad…`, reused (`containers/` unchanged). It ran from 16:21Z to 16:31Z with `exit 0`. Sites a / b / c are now `9684ae63` / `c58c97c9` / `eadf3310`, by way of phase 1 `bf3adf79` / `5f2c12e7` / `aac2f991` (`tests/budget/raw/siteday.log`, not committed). The edges were not redeployed.

### The B0-35-shaped run

```
node tests/am1/rest/a1.mjs b0-35 --repo budget35 --chain repo:65539 --run mb1 --as-admin --agents 50 \
  --push-every 5 --seconds 300 --api-reads 5 --ref-reads 2 --pollers 0
```

The repository was new: `m3org/budget35`, `repo:65539`. The run differed from B0-35 as written in these ways:
- 50 open PRs and 50 pollers every 5 s, not 500.
- Head pushes every 5 s per PR (10/s), so the budget is exceeded.
- Every request under the admin's PAT, because `graft-m3c` has no `OPS` binding to mint installation tokens.
- The base moved by `PUT pulls/{n}/merge` at the A1 rate: 27 merged, against B0-35's 10 a minute.
- No feed pollers.

| Measure | Value |
| --- | --- |
| PRs opened | 50 of 50 |
| Head pushes | 2,211 ok, 50 timed out at the driver's 120 s (p50 2.8 s) |
| Base moves | 27 merges to `main` (`PUT pulls/{n}/merge` 200), 6 answered 409 |
| Test merges computed (site 0's shard, the leader's site) | 190 |
| Shed past the budget | 694 |
| Test-merge vCPU used | 55.339 vCPU-s / 300 s = **0.1845 vCPU** |
| Budget | 0.25 vCPU (`TEST_MERGE_BUDGET` × `standard-2`'s 1 vCPU), `withinBudget: true` |
| Computes from list fields | 0 (`listComputes` 0; `listAsked` 0, see contradiction 3) |
| `mergeable` polls (single-PR GraphQL) | 3,012: 60 `MERGEABLE`, 2,952 `UNKNOWN` |
| List reads with `mergeable` | 1,531 `read:api`, all 200 |
| Sites 1 and 2's shards | No merges (`testMerges` null): test merges run only on the leader's site |
| Leader | Site 0 throughout, term 2, no leader change |
| Members | Peak 149 MiB (f0, leader), 131 (f1), 122 (f2) by `durableObjectsPeriodicGroups`. No `exceededMemory` outcome. 58 `canceled` on site a |

Seen live during the run (`/_bed/shard_status`, then the leader's `/stats`, about 2 minutes apart):

- 16:35Z: the shard had `computed` 121, `shed` 94, and `spentMs` 15,924. The leader's `testMerges` was `{ asked 149, shed 86, shard { computed 104, shed 86, spentMs 13,788 } }`.
- About 16:37Z: the shard had `computed` 176, `shed` 316, and `spentMs` 34,048. The leader had `asked` 454 and `shed` 319.

Each first attempt's reply carries the budget, for example (Workers Logs): `prsync N 104..108 first handled 840ms {"asked":1,"budget":{"computed":41,...,"shed":0,"vcpu":0.25},"events":3,...}`.

The run's final leader `/stats` read (`testMerges.leader`) came back zeroed, and so did all three members' `/stats` at 16:40Z: each showed `restores 1` and an empty `commitMs`. Each member object had restarted after the load ended, at 16:38Z to 16:40Z, and member memory stats are in-isolate. The shard's counts, read through `/_bed/shard_status`, did not restart (same `since`), so the row's numbers come from the shard. The leader's field is a live view for the run, not the gate's source.

`mergeable` was null for 98% of polls because each PR's head moved every 5 s while only about 0.63 merges a second fit the budget. A merge is recorded only for the PR's current (base tip, head) pair, so most polls saw a newer head or a moved base. This is the "null past the budget" the design states. At journey 9's push every 2 minutes per PR, the in-memory B0-35 sheds none.

Shards torn down after the run (`INSTALL=graft-m3c python3 tests/fanout/teardown.py --all-nets`):
- `git#0#net:65539`, which this run started;
- `git#0#net:65537`, `git#0#net:65538`, and `git#2#net:65537`. These were running before this run, and the owning thread reported them as restarted after its own teardown.

### What this does not show

B0-35's gate needs 500 PRs and 500 pollers on App tokens, at journey 9's push rate, with Actions dispatch p99. This run shows only that the budget is charged, sheds, and reports, and that the driver reads it. B0-35 itself still waits on the 500-agent bar (F65) and on Actions on a test install.

## Cost

$0.27 from 16:20Z to 16:45Z (`INSTALL=graft-m3c python3 tests/fanout/m2bcost.py`):
- 8,764 Workers requests, 188.7 CPU-s;
- 140,290 Durable Object requests, 135,710 rows written, 8,274 active s;
- containers 401 CPU-s, $0.09.

The cap was $5.
