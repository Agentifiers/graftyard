# narrow-reads: REST and GraphQL repository reads from narrow member reads

Branch `narrow-reads`, from main 4f61b75 (then fd6e711), which carries `learner-dumps` (K9). Updates F65 (`decisions/OPEN-QUESTIONS.md`) and meets K9's flip; decision record `decisions/DECISIONS.md`, "`narrow-reads`: REST and GraphQL reads answer only the rows their reply renders". Hosted on `graft-m3c`, 2026-11-12.

## Summary

At journey 9 every REST and GraphQL repository read went through `api::read` to `read_repo(Rows::All)` and fetched and parsed the whole packed dump, about 1.1 MB on a 500-PR repository. Now a repository `GET` that `graftyard_api::narrow::rest_serves` names, and every GraphQL query but the whole-tier ones listed below, reads `{"rows": {"api": spec}}`: only the rows its reply renders, plus the read gate's access rows and the member's selections. Replies are byte-identical to the whole-tier replies.

On the same install, repository shape (500 open PRs, dump 1.08 to 1.10 MB), 8 learners, and 300 s of B0-29 at 500 agents:

| `read:api` (500 agents, 200 reads/s) | Before (whole tiers) | After (narrow) |
| --- | --- | --- |
| 200 | 43,950 | 54,516 |
| 503 | 3,271 | 85 |
| Skipped at the driver's 400 in flight | 11,945 | 4,481 |
| Served, of answered | 93.1% | 99.8% |
| p50 / p99 | 1,902 / 4,982 ms | 1,714 / 4,370 ms |
| Edge CPU per answer, GraphQL, p50 / p99 | 66 / 170 ms | 11 / 102 ms |
| Edge CPU per answer, `GET /issues`, p50 / p99 | 67 / 171 ms | 13 / 76 ms |
| Edge wall per answer, GraphQL, p50 / p99 | 2,134 / 5,126 ms | 1,620 / 4,476 ms |
| Edge wall per answer, `GET /issues`, p50 / p99 | 2,154 / 5,030 ms | 1,826 / 4,835 ms |
| Leader change during the load | none (0 → 0) | none (0 → 0) |

The edge's CPU per answer fell 5 to 6 times at the median, and shed API reads fell from 3,271 to 85. API p99 did not reach the 1 s target: it fell from 5.0 to 4.4 s. Once the edge no longer parses 1 MB per answer, the wall time is set by the request path's sequential member calls, not by the reply. An idle `GET /issues?per_page=30` on the new edge spent about 200 ms in `auth`, 500 to 600 ms in `resolve`, and 500 to 650 ms in `rows_authz` (its `server-timing`), and a GraphQL read about 400 to 550 ms in `auth_resolve` and 300 to 500 ms in `rows_authz`. So the remaining latency is most likely the token, name, and `Authz` reads that come before the tier read, and queueing on them at 200 reads/s, not the reply [inference: the idle phase timings above, not measured under load]. That is recorded on F65 as the next limit.

| Same runs | Before | After |
| --- | --- | --- |
| `read:ref` p50 / p99 | 4,645 / 13,833 ms | 2,455 / 5,628 ms |
| Agent pushes ok, p50 / p99 | 1,255 of 1,255, 7,745 / 18,703 ms | 1,256 of 1,257, 5,021 / 8,556 ms |
| Merges p50 / p99 | 13,189 / 37,352 ms | 12,105 / 20,497 ms |
| Learners' read-index reads (`/stats`, end of run) | 7,536 to 11,601 each | 7,284 to 10,843 each |
| Learners' reads shed | 706 to 2,824 each | 0 to 35 each |
| Learners' reads waited for a rebuild | 4,013 to 9,857 each | 139 to 1,048 each |
| Learners' answer builds | 454 to 544 dumps each | 121 to 267 narrow answers each |
| Leader's `/read`s | 13,082 | 12,533 |

Hosted spend in this thread: $2.84 (`INSTALL=graft-m3c python3 tests/fanout/m2bcost.py FROM TO` over 08:48 to 09:17Z and 17:58 to 19:10Z, containers included), under the $25 cap.

## The change

Commits on `narrow-reads`: e12af85 (design, cost, decisions), f95a1fb (the change, tests, and plants), 721ad59 (merge of origin/main with merge-budget-2's merge asks), 21591a8 (`tests/nr/telemetry.mjs`), 6639de9 (`tests/nr/run.sh` and `siteday.sh`), then these results.

- `crates/api/src/view.rs`: `Directory::pick(key, select) -> Pick` (items, total, more), which defaults to running `select`, so the leader's renders and every other caller are unchanged. A renderer that selects from a whole table asks for a pick: `rest::list_issues` (filter, sort, and page in one selection, keyed by the query), GraphQL connections (`gql::list`, keyed by kind and arguments) and search (`gql::search`, `Conn::window`'s semantics), `issue_count`, a pull request's review comments, and a check run by id.
- `crates/api/src/narrow.rs` (new): `Spec { at, route }` with `Route::Rest { tail, query, path, agent }` and `Route::Graphql { query, variables, op }`; `rest_serves` and `rest_get`; and `answer`, which runs the route's renderer over the member's fold through a `Recorder` that keeps every row read, except while a pick's selection runs. A `Recording` directory returns `{"whole": true}` for a lookup by login, and records the accounts whose logins the reply asks for. The answer is `TierRows` with the recorded rows, the core access rows, and the picks, including `logins`, the accounts the reply names that the whole tiers name, and `login_shards`, the lowest `LOGIN_SHARDS_MAX` name shards of every named account. With these, the edge reads exactly the logins a whole-tier read's edge did. It answers `{"whole": true}` for a mutation, a by-login selection, or a route it does not serve, and `{"error": ..}` for a spec it cannot parse.
- `crates/api/src/hosted.rs`: `TierRows.picks` (absent unless set, so every other answer's bytes are unchanged); `HostedDir.picks` and its `pick`.
- `workers/site/src/family.rs`: `Repo::read_json` answers `{"rows": {"api": spec}}` through `narrow::answer`, with the named accounts cached per `DumpKey`. `narrow_bytes` caches answers per repository, `DumpKey`, and query text, up to 32 per state; `narrow_wait` builds one query's answer at most every `NARROW_EVERY_MS` (200 ms [guess]); `follower_query` serves api reads only with the metadata tier loaded; `forget_dumps` clears the narrow cache too.
- `workers/site/src/member.rs`: `waits_for_rebuild(learner, narrow, ..)`: a follower holds a narrow read whose build is not due, within `FOLLOWER_WAIT`, as a learner holds a dump read.
- `workers/edge/src/api.rs`: `Rows::Api(Route)` and `api_rows`, a `read_spread` of `{"q": {"rows": {"api": spec}}}` to learners, then followers. `read_repo` falls back to `tier_rows` on a whole or error answer. `read_logins` takes the picks' accounts. `rest_narrow` serves the narrow `GET`s, including `commits/{ref}` through the git cache, and returns `None` when the render asked for a pick the member did not make, so the caller reads the whole tiers. GraphQL queries read `Rows::Api` and reread `Rows::All` on a missed pick.

No wire enum changed. The new query shape is a JSON read body, and an older member answers it as an unknown read, which the edge treats as `whole`.

### Routes that still read the whole tiers

- GraphQL queries that select by login: `filterBy` (`createdBy`, `assignee`, `mentioned`) and search qualifiers such as `author:`, `assignee:`, `review-requested:`, which read the owner's names through `Directory::by_login`.
- Every GraphQL mutation and every REST write (`POST`/`PATCH`/`PUT`/`DELETE` on repository routes): their plan reads the fold the command sees.
- Repository routes `rest_serves` does not name: `collaborators`, `hooks`, `PATCH`/`DELETE /repos/{o}/{r}`, release assets and uploads, LFS, codeload, labels writes, task, lease, and agent-policy routes under `/api/graftyard/v1`, and any route not listed in `narrow::rest_serves`.
- Any read whose member answers `whole` or `error`, or whose render misses a pick (a member of an older release).

The agent push (`{"rows": {"agent": true}}`), the read gate (`access`), and merges (`merge`) were already narrow and are unchanged.

## Tests and planted bugs

- `crates/api/tests/narrow.rs` (8 tests), on a 40-item repository whose creation, update, and comment orders are permuted against number order, with closed items, labels by `n % 4`, review comments, check runs and statuses on two commits, releases, an agent's pull request, and a review request of an account no other row names (on a named account's name shard):
  - `rest_issue_lists_are_the_whole_tier_bytes_and_hold_only_their_page`: 480 `GET /issues` queries over every state, sort, direction, label filter, and page boundary. Each narrow reply is byte-identical to the whole-tier reply. The page's numbers and the presence of a `rel="next"` link are checked against an oracle that applies GHES's documented rules to the raw rows, independently of `Repo`, and the narrow rows hold exactly the page's items.
  - `single_object_routes_read_only_their_object`: `GET /pulls/2` holds no other item's rows and only #2's head; review comments, check runs, and statuses hold only their own rows; the access rows match both tiers'; `check-runs/999`, the repository, `tarball`, and releases are byte-identical.
  - `graphql_connections_are_the_whole_tier_reply`: B0-29's `pullRequests(last: 10)` holds 10 items, not 40, and returns the oracle's numbers. Two connections in one document get their own selections. Every captured gh list document is run at 6 page sizes and cursors and 3 state sets, with nodes, `totalCount`, and `hasNextPage` checked against an independent oracle.
  - `a_selection_by_login_or_a_mutation_reads_the_whole_tiers`; `a_narrow_answer_is_the_state_it_was_read_at` (a later entry changes the answer); `the_logins_are_the_whole_tier_reads_for_the_accounts_the_reply_names` (an account named only by a review request is not read; a viewer the tiers do not name is); `the_trust_view_over_narrow_rows_is_the_whole_tier_view` (and `is_agent` for #2's `agent_meta` row); `a_selection_the_member_did_not_make_is_known`.
- Site: `fanout::tests::a_narrow_read_is_cached_per_query_and_state_and_spaced`; `member::tests` for the follower wait.
- `python3 tests/nr/plant.py`: 29 of 29 caught. The plants cover wrong page boundaries (page cut before the sort, page 2 one item early, `more` from the page, a cursor page that includes its cursor, `totalCount` of the page, search `last` taking the first n); filters applied after paging (labels after the page, state dropped from the selection); pick keys that ignore the query or arguments; a single-object read leaking other rows (a selection recorded, a scan that keeps the next commit's statuses, review comments or a check run not picked, access rows left out); by-login and mutation reads answered narrowly; logins of unnamed accounts sent, or the viewer left out; the trust view's rows not recorded; a missed pick rendered without a whole read; and stale reads (the cache ignoring the tier state, spacing a cached answer, serving while the metadata tier loads, waiting past the deadline, and a follower that never waits). The first run caught 17 of 29, because each test compared two renders of the same mutated code. Every gap is now closed by the independent oracles above, a same-shard unnamed account, and an `AgentMeta` row.
- `python3 tests/lm1mem/plant.py 44 45`: the two follower-wait plants this branch moved, both caught.
- `crates/api/gh/validate.py` against the pinned `ghes-3.22.json` (sha256 b5723858…) and `schema.docs-enterprise.graphql` (sha256 fe9c21b8…): 80 GraphQL and 495 REST narrow replies (`GRAFTYARD_REPLIES=dir cargo test -p graftyard-api --test narrow`), 0 failed.
- `cargo test -p graftyard-git --test agent_bytes` passes, and the trust view (LF-4) is covered by the narrow trust test and the existing `surfaces.rs` byte-diff test.
- Full checks on 721ad59 (after merging origin/main): `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, wasm32 clippy for site and edge with and without `test-faults` and for ops, `cargo test --workspace` (75 suites, 0 failed), `go test ./...` in `containers/git/server`, and the three `scripts/check_*.py`.

## Design and cost

- `design/DESIGN.md`: "Follower read" describes narrow API reads, the member-side selections, the answer cache and `NARROW_EVERY_MS`, and the routes that still read whole tiers; journey 9, item 1; Scale ("Hot group reads, learners"); and Cost.
- `scripts/cost_agents.py`: `API_READ_WEIGHT` 1 replaces `LEARNER_TIER_READS` 20, so `GroupLoad` counts an API read once instead of 50 times. A1's 322 reads/s stay under `LEARNER_AT`, so the learner line is $0 at every tier (it was $0, $16, and $237), and the agent layer is $21, $274, and $2,572 a month (9%, 23%, and 28% of the base), from $21, $289, and $2,808. `research/costs.md` was regenerated by `scripts/costs_report.py`, and `README.md` and `design/AGENT-LAYER.md` copy the figures. The weight is an [inference]: under the narrow reads, 8 learners built 121 to 267 narrow answers each and shed 0 to 35 reads each in 300 s, against 454 to 544 dumps and 706 to 2,824 shed before. Learner CPU was not separable per learner in the Analytics DO groups, so the hosted run does not measure that a narrow read costs a learner what any other narrow read costs.
- `decisions/DECISIONS.md`: the new entry; K9's flip line now notes it was met. `decisions/OPEN-QUESTIONS.md`: F65 and K9 updated. `build/BUILD-PLAN.md`: AM1-WP6's note.

## Runs

Both runs follow `tests/nr/run.sh`, which is `tests/ld/run.sh` with retries: a fresh repository with 500 open PRs (B0-35's setup), learners placed with `node tests/fanout/learners.mjs repo:ID enam:4000,weur:2000,apac:2000` (8 learners: enam 4, weur 2, apac 2), then `a1.mjs b0-29 --seconds 300 --as-admin`.

| | Before | After |
| --- | --- | --- |
| Sites and edge | merge-budget-3 0ddc1c3 (main + hot-shards fcc604f + merge-budget-3), test-faults, image sha256:7e0c4772…; edge 54ddce4d | `nr-deploy` 2d63c4e (narrow-reads 6639de9 + hot-shards fcc604f + merge-budget-3 0ddc1c3), same image; edge e5524eb7 with `--ops graft-m3c-ops` and FORMATS with 32.18 |
| Repository | m3org/nrb1, repo:65557 | m3org/nra1, repo:65558 |
| Dump at load start | 1,099,674 B at index 1080 | 1,080,680 B at index 1038 |
| Load window | 18:23:46 to 18:28:46Z | 18:59:04 to 19:04:04Z |
| Rows | `tests/nr/rows/a1-b0-29-nrbefore2.jsonl` | `tests/nr/rows/a1-b0-29-nrafter.jsonl` |

`python3 tests/ld/summary.py tests/nr/rows/a1-b0-29-nrbefore2.jsonl tests/nr/rows/a1-b0-29-nrafter.jsonl` prints the `read:api`, `read:ref`, and push lines above. Edge CPU and wall come from `node tests/nr/telemetry.mjs graft-m3c-edge FROM TO PATH` (Workers Observability, `$workers.cpuTimeMs` and `$workers.wallTimeMs` over each load window, filtered to `/api/graphql` or the repository's `/issues`). Learner counters are `node tests/ld/lstats.mjs` at the end of each run, in `tests/nr/rows/run-*.status`.

Both fresh groups had no leader after setup: all three voters were followers at term 2 with the log committed. The learner placement retried for about a minute, then a test-only `/campaign` on site 0 answered `resumed`, and that member led at once and through the load. This is F67's leaderless group (thread f67-liveness), not a leader change during a run. It was reported to that thread. An earlier "before" run at 08:49Z (repo:65552, edge 14cc4626) got no learners for the same reason and is kept as `tests/nr/rows/a1-b0-29-nrbefore-nolearners.jsonl` (10.5% served, p50/p99 1,443/3,137 ms); it is not the baseline. The first attempt at 17:58Z failed at create on `NEAR_RTT` (`run-nrbefore2-placefail.status`).

## What is left

- API p99 of 4.4 s against 1 s. The edge's reply work is gone, and the remaining wall time is the request path's sequential reads before the tier read (token, name, and `Authz` by read-index) and their queueing at 200 reads/s (F65).
- `NARROW_EVERY_MS` (200 ms) and `NARROW_PER_STATE` (32) are guesses.
- `API_READ_WEIGHT` 1 is an inference; per-learner CPU under narrow reads is not yet measured.
- LM-1's 500-agent rows on `graft-m2b` rerun this (thread T-01a1c694-6112-72b4-83ca-dc9337a9646e).
