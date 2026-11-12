# `merge-budget-3`: REST polls compute, counts outlive the shard object, and the depth error is gone (2026-11-12)

Branch `merge-budget-3`, from `origin/main` `eb2355b` (merge-budget-2 merged), with `origin/hot-shards` merged in. It closes the five items the lead left after `merge-budget-2` (`tests/am1/results/merge-budget-2.md`). Hosted on the test install `graft-m3c` (`tests/am1/results/install-m3c.md`). Nothing ran on `graft-m1-*` or `graft-m2b-*`.

## Summary

- **REST polls compute (item 1).** `GET pulls/{n}` asks the shard as GraphQL's single-PR read does. It keeps `mergeable: null` until the merge is recorded, then answers it. Hosted: 96 REST polls answered `mergeable: true` in run 1, and 33 in run 2.
- **`standard-4`, hosted (item 2).** On a `standard-4` shard the budget is 1 vCPU. 50 PRs with heads pushed every 30 s used 0.111 vCPU. 927 merges were computed, 370 of them for GETs, and 0 were shed. `SHARD_INSTANCE` is back to `standard-2`.
- **Concurrency bound, hosted (item 3).** First attempts were cut to 3 s, so 47 of them timed out while their merges ran on and the queue's re-sends started. Up to 69 merges were in line at once. The bucket's lowest was −24.8 ms against a largest single merge of 37.8 ms, so it stayed within one merge. The spend was 0.191 vCPU of 0.25.
- **Counts persist (item 4).** The shard's counts read the same 127 s after its container was torn down, and again after a site day had reset every object.
- **Depth error gone (item 5).** It came from the first attempt running in the leader member: each merge's catch-up read back into the member and pushed its next call one hop deeper, until the eighth `/merge` failed. A probe reproduced it 3 of 3 times. The first attempt now runs in a new invocation through the site's own service binding. Hosted, both runs started on cold shards and logged 0 "Subrequest depth limit exceeded"; merge-budget-2's runs logged 16 and 4.
- **Plants.** 45 of 45 caught.
- **Cost.** $0.43 for the hosted window (17:15Z to 17:56Z), $0.12 of it containers.

## 1. REST `GET pulls/{n}` asks for its merge

Agents and CI poll REST `pulls/{n}` far more than GraphQL, and merge-budget-2 left it silent: its renderer is the webhook's, which always says `mergeable: null`.

- `graftyard_api::gql::test_merge(r, it)` is now the one place that reads a PR's recorded merge and decides what to ask: the recorded result when it is of the current head and base tip, and otherwise, for an open same-repository PR, the (base tip, head) to merge. GraphQL's `Node` and REST both call it, so the two surfaces ask for the same merges.
- `rest::get_pull_asks` returns the reply and the asks. The edge's `(Get, ["pulls", n])` route sends them with the same `ask_merges` the GraphQL read uses, after the reply: a writer's or an App's poll as `/demand?cause=get`, anyone else's as a list's count (`merge_computes`).
- Until the merge is recorded, the reply is GHES 3.22's `mergeable: null` and `mergeable_state: "unknown"`. Once it is, `mergeable` is the result, and `mergeable_state` is the lower case of GraphQL's `mergeStateStatus` (`gql::merge_state`, moved out of `Node` so both use it): `clean`, `blocked`, `dirty`, or `draft`. A base that moves past the record makes both null and unknown again. `rebaseable` stays null [unverified: what GHES 3.22 answers beside a computed `mergeable`].

## 2. `SHARD_INSTANCE=standard-4`, hosted

The site starts its shard container at `SHARD_INSTANCE` (`ShardHost.instance`, passed to `container.start`, since the site's containers use the `durable_object` scheduling policy) and sizes `Budgets` from the same name. `tests/am1/mb3/siteday.sh` takes the instance as its second argument and passes it with `--vars`.

Run 1 ran on `SHARD_INSTANCE=standard-4` (site day 17:16Z to 17:24Z). The shard reported `vcpu: 1.0`, so its budget was 1 vCPU, as journey 9 sizes it:

- 50 PRs, heads pushed every 30 s (4 times journey 9's per-PR rate), for 300 s: **0.111 vCPU** of 1.0, with 927 merges computed and **0 shed**.
- GETs had headroom. Of 1,154 GET asks, 370 computed and the rest coalesced per PR; none were shed.
- Journey 9 sizes head-push merges at 0.21 vCPU of a `standard-4`'s 1 [measured, B0-34]. This run, at a higher per-PR rate on small repositories, used half that with GETs included, so the sizing holds with room for GETs.

I did not read the container's instance type from the Cloudflare API before teardown. That it ran on `standard-4` rests on the code: the same `SHARD_INSTANCE` that set the budget is passed to `container.start` [inference]. Site day 2 (17:39Z to 17:42Z) restored `standard-2`.

## 3. Concurrent re-sends, hosted

Round 5's overrun came from first attempts that timed out at 25 s while their queued re-sends started, so several consumer handlers sent `/merge` at once (`tests/am1/results/merge-budget-2.md`, "A28: the cause").

- Under `test-faults`, `/bench_seed {"prsyncFirstFor": ms}` sets a member's first-attempt deadline (0 clears it). The attempt it gives up on is not cancelled: it runs on in its own invocation (item 5), so its merges and the re-send's reach the shard together. `a1.mjs --first-for MS` sets it on all three members after the stats reset and clears it after the load.
- `BudgetStats.in_flight_max` is the most merges at once that passed the first look at the budget and were catching up, waiting for the turn, or running. Above 1, merges arrived concurrently and the turn put them in line. A Drop guard leaves the count however `asked` ends. The driver reports it as `inFlightMax`.

Run 2 ran on `standard-2` (0.25 vCPU) with `--first-for 3000`:

- 47 first attempts gave up after 3 s; 11 answered within it. Each abandoned attempt kept merging while the leader re-sent its batch through the queue (24 consumer log lines), as in round 5.
- `inFlightMax` reached **69**: merges from abandoned attempts, re-sends and GETs all waited for the one turn.
- The lowest bucket was **−24.8 ms**; the largest single charge was 37.8 ms. "No overrun beyond one merge" is `lowMs >= -max(maxMs, 50)`, and it held. It held against `maxMs` alone too.
- 1,783 merges were computed and 648 shed, for 0.191 vCPU over the window against 0.25.

Run 1 also shows concurrency without the fault: `inFlightMax` was 9 from pushes and GETs alone, and the bucket never went below zero.

## 4. Counts outlive the shard object's reset

The shard object resets about 90 s after its last request, so merge-budget-2's run 1 read its counts from Workers Logs.

- The shard stores each network's `BudgetStats` under `budget:<network>` after every list ask, every coalesced GET, and every merge result (`save_budget`), before the reply that carries them leaves.
- Before `/status`, `/ask`, `/demand`, or a budget `/merge`, it restores them the first time the object sees the network (`restore_budget`, `Budgets::restore`, `MergeBudget::restored`). A restore never overwrites a network the object already budgets, and takes the shard's vCPU now, not the stored one.
- The bucket itself is not stored. A restarted object starts it full, as before: the bucket holds one second of refill, and the object resets only after about 90 s idle.

Hosted on repo 65553, after run 1:

| Read | When | Container | `computed` | `getAsked` | `listAsked` | `spentMs` | `vcpu` |
| --- | --- | --- | --- | --- | --- | --- | --- |
| After the load | 17:36:26Z | running | 973 | 1,154 | 1,846 | 34,993 | 1.0 |
| Container torn down at 17:38:17Z, no request since | 17:40:24Z | stopped | 973 | 1,154 | 1,846 | 34,993 | 1.0 |
| After site day 2, which reset every object | 17:42:51Z | stopped | 973 | 1,154 | 1,846 | 34,993 | **0.25** |

The idle read came 127 s after the last request, past the roughly 90 s reset. It does not prove the object had already reset by then; the site day's new version certainly did. After that reset the counts came back from storage. `vcpu` changed to the new `SHARD_INSTANCE`'s, because a restore takes the shard's size now. Raw reads are in `tests/am1/raw/mb3/shardstatus-*.jsonl` (not committed).

## 5. "Subrequest depth limit exceeded" on a cold shard's first catch-up

### The call chain

- Trace `62b928c0` on site a (Workers Logs): the leader's alarm started a `PrSync` first attempt in its own isolate (`spawn_local`, merge-budget-2's `prsync::on_alarm`). The attempt sent `/merge` for n = 43 to 49 one after another. Each `/merge` was answered only after the shard read back from the leader member (`/read`, `/entries`) to catch up. The eighth `/merge` failed "Subrequest depth limit exceeded".
- Runs `77603e9e` (merge-budget-2 run 2) and `b3ec102d` (2026-11-11) fail at the same count, the eighth merge.
- The cause is the runtime's accounting, which m3-wp5 found for Raft calls (`tests/m3/results/m3-wp5.md`, "Where the depth comes from"): a Durable Object's outbound calls are charged to the newest incoming request it holds. The attempt ran in the member, and every merge's catch-up sent the member a new, deeper request (the shard's `/read`). So each merge started one hop deeper than the last.
- Alarm chaining is not the cause: the trace starts at the alarm, and the depth grows with the merges inside one attempt.

### The probe

A probe Worker `graft-mb3-depth-probe` (`tests/am1/mb3/depth-probe.rs`, Rust, worker 0.8.7; deleted after use) has a leader object in `weur` and a shard object in `enam`. The leader loops over "merges": each is a call to the shard, which calls back into the leader before it answers.

| Loop | Runs | Result |
| --- | --- | --- |
| In the leader object's `spawn_local`, a new shard stub per merge, as `prsync::shard_post` | 3 | **failed at the 8th** in 3 of 3 |
| The same loop in a stateless invocation, through a service binding to the probe's own script | 3 | **ok, 20 of 20** in 3 of 3 |
| In the leader object, one reused stub | 3 | ok |

### The fix

- The alarm now sends each first attempt to `POST /prsync/first` on the site's own script through a `SITE` service binding (`prsync::in_band`). The script's fetch handler (`first_handled`) runs `one()` under `ctx.wait_until` and answers its JSON, or a 500 with the error. The new invocation starts its calls at depth 1 and receives none of the shard's calls back, which go to the member.
- `deploy.mjs` adds `services: [{ binding: "SITE", service: <the site script> }]`, once the script exists.
- The fetch handler has no other route, and `workers_dev` and preview URLs stay off, so nothing outside the script reaches it.
- Re-sends still go through the queue. The rejected alternatives: sending first attempts through the queue too, which `2026-11-05` rejected for its latency; and reusing one stub per attempt, which passed the probe but depends on how the runtime picks the request to charge.
- A service binding call is not billed as a request [doc, Workers pricing], so cost does not change.

### Hosted

Both runs started on fresh repositories, so each shard's first catch-up was cold. Workers Logs for `graft-m3c-site-a`, `-b`, `-c` and `graft-m3c-edge` hold **0** "Subrequest depth limit exceeded" for 17:24Z to 17:36Z and 17:42Z to 17:53Z. merge-budget-2's runs logged 16 and 4 in their first seconds. Every first attempt logged `POST https://site/prsync/first`, then `first handled` with the merges it asked. One run 1 attempt asked 23 merges in a single call, about three times the 8 that failed before.

## Tests and plants

- `crates/api/tests/surfaces.rs`, `a_rest_pull_read_asks_for_the_merge_graphql_asks_for_and_keeps_mergeable_null`: REST asks for the same (base tip, head) GraphQL does, an issue or unknown number asks nothing, a moved base names the new tip; once recorded, nothing is asked and REST answers `true`/`clean`, then `true`/`blocked` while the required check re-runs, then `false`/`dirty` for a conflict at a newer tip, each equal to GraphQL's `mergeStateStatus`; a base past the record is null and `unknown` again, and asks.
- `crates/git/tests/agent_load.rs`:
  - `concurrent_merges_overrun_the_budget_by_at_most_one_merge_and_are_charged_their_cpu` now asserts `in_flight_max` is 5 to 10 (it measures 8).
  - `a_restarted_shard_goes_on_from_its_stored_counts_and_counts_merges_in_line`: a restored budget keeps every count and `since`, takes the new shard's vCPU (1.0 on a `standard-4`), starts full; a second restore of older counts does not undo newer ones; another network stays unbudgeted; sequential merges count `in_flight_max` 1.
- `tests/am1/rest/a1.mjs --selftest`: `inFlightMax` is the most of any site's, and 0 for an older shard.
- **Plants: 45 of 45 caught** (`python3 tests/budget/plant.py`): merge-budget-2's 35, two retargeted at the moved `test_merge` lines, and 10 new:
  - REST asks nothing;
  - REST never renders a computed merge;
  - REST renders a computed merge as clean whatever blocks it (missed at first: the seeded PR's required check passed, so `clean` and `blocked` agreed; the test now re-runs the check);
  - REST renders a stale merge, ignoring the base tip;
  - a restore overwrites newer counts; a restore drops the counts; a restore keeps the stored vCPU;
  - `in_flight_max` never counted; `in_flight` never left; the driver drops `inFlightMax`.
- Not planted, because no local test reaches it: running the first attempt in the member again (item 5), and skipping `save_budget` (item 4). The hosted runs cover both: no depth error, and counts read after the reset.

Full checks, all pass:

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo clippy --target wasm32-unknown-unknown -- -D warnings` in `workers/site` and `workers/edge`, each with and without `--features test-faults`, and in `workers/ops` (which has no `test-faults` feature)
- `cargo test --workspace`
- `go test ./...` in `containers/git/server`
- `scripts/check_design.py`, `check_names.py`, `check_links.py`

No wire enum changed. `/prsync/first` carries the queue's `Message` JSON, and the stored counts are `BudgetStats` JSON.

## Hosted on `graft-m3c`

### Site days

- Branch `merge-budget-3` `0ddc1c3`: origin/main plus `origin/hot-shards` `fcc604f`, which brings the inactivity-timeout fix and the edge's fallback to other sites' shards. test-faults, hot-shards' image `sha256:7e0c4772…`, deployed with `tests/am1/mb3/siteday.sh`.
- Site day 1, 17:16Z to 17:24Z, `SHARD_INSTANCE=standard-4`. Site day 2, 17:39Z to 17:42Z, `standard-2`.
- `graft-m3c-edge` is `54ddce4d`, built from the same tree, with `OPS = graft-m3c-ops` and `FORMATS_ENABLED` including 32.18.
- Coordinated after hot-shards and m0-carried-4, with narrow-reads next. m0-carried-4's own site change (`09b2a2d`) was not in this build.

### Runs

```
bash tests/am1/mb3/hosted.sh mb3-s4 mb3r1 50 30 300 --rest-polls 0.3                  # repo:65553, standard-4
bash tests/am1/mb3/hosted.sh mb3-ff mb3r2 50 10 240 --rest-polls 0.3 --first-for 3000  # repo:65554, standard-2
```

Both runs use 50 open PRs, one per agent, on App tokens. 70% of the `mergeable` polls are single-PR GraphQL and 30% are REST `GET pulls/{n}`, alongside list reads with `mergeable` and base moves by `PUT pulls/{n}/merge`.

| Measure | Run 1, `standard-4` (17:29:34Z, 300 s) | Run 2, `standard-2`, first attempts cut to 3 s (17:47:20Z, 240 s) |
| --- | --- | --- |
| Budget | 1.0 vCPU | 0.25 vCPU |
| Test-merge CPU | **0.111 vCPU** | **0.191 vCPU** |
| Merges computed, shed | 927, 0 | 1,783, 648 |
| Lowest bucket, largest merge | 0.0 ms, 59.3 ms | **−24.8 ms**, 37.8 ms |
| Overrun beyond one merge | No | No |
| Most merges in line at once (`inFlightMax`) | 9 | **69** |
| GET asks, GET merges computed | 1,154, 370 | 1,303, 150 |
| List asks counted, list computes | 1,846, 0 | 2,184, 0 |
| GraphQL polls | 242 `MERGEABLE`, 789 `UNKNOWN`, 1,068 HTTP 503 | 81 `MERGEABLE`, 946 `UNKNOWN`, 681 HTTP 503 |
| REST polls | 96 `mergeable: true`, 353 null, 432 HTTP 503 | 33 `mergeable: true`, 357 null, 261 HTTP 503 |
| Merge CPU / wall | 0.30 | 0.23 |
| Agent pushes | 412 ok of 500 | 1,083 ok, 35 refused |
| Leader | site 0, term 2, commit p99 830 ms | site 0, term 2, commit p99 7,578 ms |
| "Subrequest depth limit exceeded" | 0 | 0 |

Every count is the driver's window, from the shard's own stored counts (`/_bed/shard_status`).

Not caused by this change, recorded for the owners:

- **HTTP 503s on polls** ("the repository's tier rows did not answer; retry") are the read path's load shedding (F65), as in merge-budget-2. The asks run after the reply, so they cannot change a reply's status [inference].
- **Run 2's commit p99 of 7.6 s** (queue p99 7.2 s) came under a fault that runs every batch twice. Each abandoned attempt and its re-send commit the same `PrSync` commands, idempotent by request id, so the leader's queue carries about twice the `PrSync` commands [inference; not separated in the logs]. Run 1, without the fault, had p99 830 ms.

### Teardown

After the runs `INSTALL=graft-m3c python3 tests/fanout/teardown.py --all-nets --pools` tore down `git#0#net:65554`. It also tore down three shards left running from earlier windows: `git#0#net:65551`, `git#1#net:65551` and `git#0#pool:6`. A second pass found none running. Run 1's shard was torn down at 17:38Z for the persistence read.

## Design changes and contradictions

1. **Agent block** (`design/DESIGN.md` and `design/AGENT-LAYER.md`, byte-identical): merges run "on a single-PR read, GraphQL or REST"; the budget's "counts are stored, so they outlive the object's reset".
2. **DESIGN, "Dominant access patterns", Webhook:** `PrSync`'s first attempt runs "at once in a new invocation of the leader's site script through its own service binding", with the depth evidence. `design/SHARED.md`, "Event fan-out", says "at once, without a queue hop" in place of "in its own isolate".
3. **`decisions/DECISIONS.md`**, "`merge-budget-3`": three entries (REST asks; counts stored; first attempt through the site's own binding). merge-budget-2's A30 entry is marked reversed on REST, which met its flip line.
4. **Contradiction, fixed:** merge-budget-2's `MergeBudget` doc said "a restart starts it full [inference]" for the whole budget, while B0-35's gate reads the counts after the load. Now only the bucket starts full.
5. **Contradiction, fixed:** DESIGN and SHARED said the first attempt runs "in the leader's isolate", which is what made the depth ratchet. The design now says where it runs and why.
6. **Contradiction, open:** `inFlightMax` 69 means a merge can wait behind many others in its turn. A28's flip line names "a merge's wait past `GET_COALESCE_MS`" as the reason to admit a second merge. These runs did not measure per-merge wait, so that flip is untested. The cheap measure is a wait p99 in `BudgetStats`.

## Cost

$0.43 for the hosted window, 17:15Z to 17:56Z (`INSTALL=graft-m3c python3 tests/fanout/m2bcost.py 2026-11-12T17:15:00Z 2026-11-12T17:56:00Z`): 13,863 Workers requests, 243,964 DO requests and 2,067 container CPU-s, $0.12 of it containers. The depth probe Worker cost less than a cent. The cap was $25.

No design cost moved. The service binding call is not billed as a request [doc, Workers pricing]. The stored counts add one storage write per ask or merge on the shard object, which `scripts/cost_agents.py` does not price separately [guess: within the shard object's request cost at journey 9's rates]. `scripts/cost.py` and `cost_agents.py` were not rerun, since no constant changed.
