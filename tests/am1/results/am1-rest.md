# am1-rest: what LM-1 still lacked, built and tested in memory (2026-11-09)

Branch `am1-rest`, cut from main `debd398`. Nothing was deployed: the `wire-fix` branch has not landed, so every
hosted run below is a plan, not a result. No stored enum gained or lost a variant (`RepoEvent`, `RepoCmd`, `IssueOp`,
and the Authz, Identity, Names and Tokens enums are unchanged). The ref-read paths `wire-fix` owns (`shard.rs`
`core_refs`, the edge's `read_refs`, `Repo::read_json`) are untouched. No record format or tag was taken: the
repository 32.15 and Authz 0x12.9 reserved for this work are still free.

The source is `build/BUILD-PLAN.md`, AM1-WP6, "What LM-1 still needs", and the open rows in
`tests/am1/results/am1-hosted.md` and `am1-fix.md`.

## Commits

| Commit | Piece |
| --- | --- |
| `2568a09` | `graftyard_fold::archive_due_walk`: `archive_due` over a paged log with a per-range byte cap |
| `b41bb53` | 1. The hosted `LOG_MAX_BYTES` archive alarm |
| `b2a5118` | 2. Install-owned Graftyard-run and MCP Apps (`tests/am1/rest/apps.mjs`) |
| `b75a454` | 4. B0-36's write surfaces: ready-for-review, convert-to-draft, `issues/{n}/labels` |
| `edd229a` | 4. B0-37's fetch half: session refs listed only to a v2 `ref-prefix` |
| `0b2bf82` | 3. The journey 9 / A1 load driver (`tests/am1/rest/a1.mjs`) |
| `8e668a7` | 5. LF-13 on real members, and the hosted LF-13 and LF-4 drivers |
| `8e1bf69` | Plants (26, all caught), the three tests the first plant run asked for, and the per-run cost script |

## 1. The archive alarm

`crates/git/src/log_archive.rs` holds the decisions. `workers/site/src/archive.rs` is the site host, hooked into the
repository leader's alarm after `snap::on_alarm` (`workers/site/src/family.rs`).

- **When.** Each leader alarm starts one run unless one is in flight. It sums the stored log above the last archive at
  most once a minute.
- **Due.** `due` cuts a log over `LOG_MAX_BYTES` down to `cap - cap/16` (`ARCHIVE_SLACK`), in ranges of at most
  `ARCHIVE_RANGE_MAX` (2 MiB [guess]). It runs only once both tiers have a snapshot, and only on committed entries. It
  refuses a stored log with a gap, or a row that holds a different entry from the one its index names.
- **Promote, then archive.** `next` promotes a due range that no `Promoted` covers. The leader's own-site shard runs
  `/promote` over the range's packs (SHA-256 of each inline pack, the stored checksum of each stored one) and then
  `/validate`. `PackStore::place` puts the result, the leader records `Promoted`, and the markers are put again. Once
  a `Promoted` covers part of the range, that part is archived: `archive_object` builds it, it is put create-only to
  two buckets of the record's sites, and `Archived` is recorded. Members truncate through it once they can HEAD the key
  in two buckets (`Host::truncate`, unchanged). After that the install source names it, so archived entries stay
  readable for `RestoredTo` and `InstallSnapshot` (`crate::load`, unchanged).
- **Idempotent.** The request ids come from the range (`promote_request`) and from the object's digest
  (`archive_request`), so a re-sent step records nothing twice.
- `Repo::row_packs` now names a `Promoted` entry's pack, so the sweeper never takes it for an orphan.
- `Stats.archive_steps` is shown as `archiveSteps` in member `/stats`.
- On test installs only, `/bench_seed {"archiveCap": {"max": n, "range": n}}` sets a small cap for the isolate.

**In memory.** `crates/git/tests/log_archive.rs` has 7 tests on three real `Member<RepoCmd>` of the common World, with
the shard and three buckets stubbed:

- 626 entries gave 108 promotions and 108 archives. The peak stored log was 45,868 bytes against a 40,000 cap.
- Every archived entry reads back byte for byte with its digest chain, and by time.
- A re-sent step records nothing twice.
- An archive held in one bucket is never recorded.
- One tier's snapshot alone archives nothing.
- The slack cuts below the cap.

The site's own tests (`workers/site/src/archive.rs`, 2) check that the `Archived` the alarm records is the range
members truncate through, and that a `Promoted` row names its pack.

## 2. Install-owned agent Apps

`tests/am1/rest/apps.mjs` stands in for `graftyard apps init` / `agents init`. It registers three Apps on `authz:1`
(`install_account()`):

| App | Kind | Permissions | Installation |
| --- | --- | --- | --- |
| `graftyard-workspace` | `GraftyardRun` | `contents: write` | 7190011 |
| `graftyard-agent` | `GraftyardRun` | the read-heavy set | 7190012 |
| `graftyard-mcp` (`Graftyard MCP`) | `Mcp` | `contents`, `issues`, `pull_requests` write | 7190013 |

The `graftyard-agent` and `graftyard-mcp` permission sets are the ones in the AGENT-LAYER "First-party Apps" table.

For each App the script sends, in order:

1. `ClaimSlug`.
2. `PutApp`, with a fresh RS256 key stored 0600 under `$M3_STATE/agent-apps/<slug>.pem` and never printed.
3. `RegisterApp` and `RegisterClient`.
4. `RegisterInstallation`.
5. `Install` on the org.
6. `PutAgentApp`.

`agentToken(slug, extra)` mints that App's installation tokens for the drivers and registers each one with `secret`.

**In memory.** `crates/identity/tests/agent_apps.rs` runs `apps.mjs --emit` and commits the JSON on real members. It
checks all three `AgentApp` rows, and that a rerun writes no event.

## 3. The A1 load driver

`tests/am1/rest/a1.mjs` is one script for the rows B0-29, B0-31, B0-32, B0-33, B0-34, B0-35 and L0-12. It runs from
the orb, as an orb service, through the edge:

- **Agent pushes.** Each agent pushes its own ref every 120 s to `git-receive-pack`. Packs are built in-process (one
  blob, its trees, a commit on `main`), 40 KB mean with 5% at 1 MiB. Pushes go under m3-ci's installation token,
  flagged `third_party` with `confine refs/heads/claude/**`, so they are Agent class and carry a `PathSet`.
- **Agent side commands.** These follow the A1 mix in `cost_agents.py --a1`: task edits, comments, labels (the new
  REST route), draft PRs, and lease acquire, renew and release. Commands the edge has no route for (`RequiredCheck`,
  check runs: 3.33/s) are replaced by task edits at the same rate, so the group sees the row's 27.6 commands/s.
  `PrSync` and test merges are the host's own (8.34/s).
- **Humans and merges.** Humans push and comment, and admin PRs merge through `PUT pulls/{n}/merge` at 10 a minute.
- **Reads.** 33 v2 `ls-refs` ref reads/s, 200 API reads/s, and, for B0-34, clone time to first byte.
- **Feed.** 500 long-pollers go straight to the learners (or followers) through the tools edge, each with its own
  token and IP hash, because one orb IP is held to FEED_POLLS' 50 per member. Each event's latency is the poll's
  return minus the push's acknowledgement.
- **Row variants.**
  - B0-31: every push 1 MiB.
  - B0-32: 500 pushes of 256 KiB in 1 s.
  - B0-33: 5,000 agents plus a user token at 100 commands a minute.
  - B0-34: clones, plus `--kill-shard-at`.
  - B0-35: 500 open PRs whose `mergeable` 500 clients poll every 5 s.
  - L0-12: a task and a Shared lease per agent (five requesters, because of TASKS_OPEN_PER_REQUESTER), with a leader
    kill (`x-graft-kill: after_quorum`) about once a minute.
- **Secrets.** Tokens come from `m3-tokens.txt`, `ghs` or mints, and go only in `authorization` headers. Every log
  line goes through `scan`.

**Offline checks.**

- `--dry` prints each row's plan and checks it against the rows. All 7 pass: B0-29, B0-31, B0-32, B0-34, B0-35 and
  L0-12 offer 27.6 commands/s (25.2 agent class) and 500 pollers. B0-33 offers 252.2 agent-class commands/s at
  5,000 agents (the A1 table's 252).
- `--selftest` checks the pack builder against `git index-pack --strict` and `cat-file`, and checks the
  report-status parser.

## 4. Session refs, ready-for-review, labels

**Session refs (B0-37's fetch half).** The edge's v0 advertisement and every v2 `ls-refs` come from the shard
container's own `git upload-pack`, so the filter is in `containers/git/server/main.go`:

- `uploadpack.hideRefs=refs/graftyard/` hides every host-owned ref from v0, v1, and `--mirror`.
- A v2 `command=ls-refs` whose `ref-prefix` begins with `refs/graftyard/sessions/` unhides that namespace for the one
  call. The reply is then cut to the session refs that begin with one of those prefixes, so a sibling
  `ref-prefix refs/` lists none.
- Objects stay fetchable by id.

**In memory.** `containers/git/server/sessions_test.go` runs with real git 2.56:

- ls-remote v0, v1 and v2 list no host-owned ref.
- `--mirror` over v0 and v2 copies no host-owned ref.
- Nine raw v2 `ls-refs` prefix cases, including `refs/graftyard/`, `refs/graftyard/other`, `.../sessions/1` (which
  lists `.../12`), and two prefixes in one call.
- `git fetch` of `refs/graftyard/sessions/12:refs/s` gets the tip. `refs/*:refs/all/*` gets `main` only.
- A fetch of a session log by id works.
- `sessionPrefixes` parses only `ls-refs`.

Finding: git 2.56's `ls-remote <url> <pattern>` sends no `ref-prefix` (GIT_TRACE_PACKET), so a client that wants to
list sessions has to send the prefix itself, as `git fetch` with a refspec does. Hosted, this needs a new shard
container image, which ships through a site day.

**Ready-for-review and labels (B0-36).**

- GraphQL: `markPullRequestReadyForReview` and `convertPullRequestToDraft` plan
  `IssueOp::Change { Draft(false | true) }`. `addLabelsToLabelable` and `removeLabelsFromLabelable` plan
  `ApplyLabels`.
- REST `issues/{n}/labels`: `POST` adds, `PUT` replaces, and `DELETE` removes all (204) or one (`/{name}`, 404 if the
  issue lacks it). An unknown name is 422. The route is in `rest_repo`, behind the read gate.
- Both reuse existing `IssueOp`s, and `decide` already refuses `ReadyForReview` and unlisted `Label` for agents
  (`agent_refusal`).

**In memory.** `crates/api/tests/surfaces.rs`, `ready_for_review_and_issue_labels_commit_and_render_as_ghes_does`,
commits each operation on the seeded chain and renders it from `&Acked`. `validate.py` against the GHES 3.22
description passes for the 8 new replies (4 GraphQL, 4 REST), and for all of the API crate's replies: 52 GraphQL,
75 REST and 11 LFS, 0 failed.

## 5. LF-13 and LF-4

**Why the day-6 LF-13 run never left a gap, shown on real members.** `crates/raft/tests/lf13.rs` has 3 tests:

- Rounds lost to the same leader leave no gap. The leader moves `next` only on an ok answer, so after the fault it
  re-sends from the follower's `matched`, and every answer is ok.
- A new leader's first round leaves a gap. The follower misses rounds, leadership moves to the other follower, and
  the lagging follower's first answer to the new leader is `ok: false`.
- A log truncated past the follower sends it an `InstallSnapshot` (`set_install`).

So the row's "dropped for 5 s so the next one leaves a gap" cannot happen under one leader. See contradiction 2.

**Hosted drivers.**

- `tests/am1/rest/lf13.mjs gap` holds 1,000 polls on F, then fails F's member calls while lease commits run and moves
  leadership to the other follower.
- `tests/am1/rest/lf13.mjs install` does the same with the leader's archive alarm capped at 4 KiB until
  `truncatedTo` passes F's applied index.
- Both then measure the held polls' return after the fault lifts, `lagging` probes every 200 ms, and lease events
  missing on F against the other voter from the same cursor.
- `tests/am1/rest/lf4.mjs` writes one payload per `HIDDEN_CHARS` range (16) into issues (GraphQL `createIssue`) and
  releases (`POST releases`). It reads them back under `AGENT_MEDIA_TYPE` and checks that `visible` is clean,
  `invisible_chars` is 2, and `body` is unchanged. A read without the media type carries no `graftyard_trust`.
- The other six trust resources have no REST read route on the edge, so the hosted run cannot reach them. The driver
  lists them as not reachable. `crates/api/tests/trust.rs` covers all eight on the route renderer.

## Plants

`tests/am1/rest/plant.py` has 26 mutants:

| Area | Mutants |
| --- | --- |
| `log_archive` | 9 |
| The site's `row_packs` and `archived` | 2 |
| `apps.mjs` kinds | 2 |
| GraphQL draft and labels | 2 |
| REST labels | 4 |
| The Go session filter | 4 |
| The A1 pack builder and `ng` parser | 2 |
| The LF-13 install premise in `crates/raft` | 1 |

- **First run: 23 of 26 caught.** Three were missed:
  - 0, no slack;
  - 8, archiving after one tier's snapshot;
  - 16, an unknown label accepted beside a known one.
- **Fix.** Each missed mutant got a test: `a_log_over_the_cap_is_cut_below_it_by_the_slack`,
  `one_tier_s_snapshot_alone_archives_nothing`, and a mixed known and unknown `POST`.
- **Rerun: 3 of 3 caught.** All 26 are now caught.

## Full checks

On `8e1bf69`, every one of these passed:

- `cargo fmt --all --check`.
- Workspace clippy with `-D warnings`.
- wasm32 clippy for the edge and the site, each with and without `test-faults`, and for ops.
- `cargo test --workspace`.
- `go test ./...` in `containers/git/server`.
- `scripts/check_design.py`, `check_names.py` and `check_links.py`.

The numbers are in "Check results" at the end of this file.

## Hosted run plan

Nothing runs before `wire-fix` lands. Everything runs on **`graft-m2b`** (never `graft-m1-*`), from one commit of
`am1-rest` merged with main after `wire-fix`.

**One site day** ships both the archive alarm and the new shard container image (session refs). Then redeploy the
edge from the same commit, with the install's `FORMATS_ENABLED` unchanged: no new format.

**Before the rows:**

1. `node tests/am1/rest/apps.mjs` (idempotent). Then flag m3-ci `third_party` with `confine refs/heads/claude/**` (the
   driver does this itself).
2. Fresh repositories for the load rows: `tests/am1/hosted/repos.mjs` style, one per row, so no run shares a log.
3. `place_learners` on each load group before its timed phase (`feed.mjs lf12` phase C does it).

The costs are upper bounds from `python3 tests/am1/rest/run_cost.py`, which prices each run's rates with
`scripts/cost.py`'s prices and per-operation figures at overage rates.

| Run | Install, group | Command | Duration | Cost |
| --- | --- | --- | --- | --- |
| B0-29 | graft-m2b, new `a1-29` | `a1.mjs b0-29 --repo a1-29 --chain repo:<id> --seconds 600` | 10 min plus about 5 min of setup | $0.50 |
| B0-31 | new group, leader moved to site 2 (apac) | `a1.mjs b0-31 ...`, 600 s with buckets up, then 600 s with `/_bed/fault` on the buckets' placement | 20 min | $1.01 |
| B0-32 | new group | `a1.mjs b0-32 --seconds 180` | 3 min | $0.17 |
| B0-33 | new group | `a1.mjs b0-33 --agents 5000 --seconds 300`; the task setup is 100 per requester | 5 min plus about 10 min of setup | $1.97 |
| B0-34 | new group | `a1.mjs b0-34 --kill-shard-at 300` (clone TTFB, warm-shard kill) | 10 min | $0.50 |
| B0-35 | new group | `a1.mjs b0-35`; 500 open PRs first (about 3 min, ten at a time) | 13 min | $0.53 |
| L0-12 A1 replay and 500 pollers | new group, 5 requesters seeded | `a1.mjs l0-12 --kill-every 60` | 10 min plus about 15 min of tasks and leases | $0.50 |
| L0-12 refusals, Graftyard-run and MCP kinds | lf1-x on graft-m2b | `rows.mjs l012` with `agentToken("graftyard-workspace")` and `agentToken("graftyard-mcp")` as the two new kinds | about 30 min | in the B0-36/37 line |
| LF-13 | new group with learners off | `lf13.mjs gap` × 3, then `lf13.mjs install` × 3 | about 18 min | $0.06 |
| LF-4 | any repository on graft-m2b | `lf4.mjs --repo <name>` | about 1 min | under $0.01 |
| B0-36 and B0-37 reruns | lf1-x | `rows.mjs b036` and `b037` with the Graftyard-run kind, plus the fetch half: v0, v1, v2 and `--mirror` against a session ref `graftyard-workspace` pushed | about 60 min | $0.19 |
| **All of the above once** | | | **about 3.5 h of wall time** | **about $5.44** |

**B0-30 is not run.** It costs about $65 and takes 40 h (`am1-hosted.md`, "B0-30 plan and cost"), and it needs the
user's OK. The archive alarm it was waiting on now exists.

**The B0-29 and LF-12 latency line.** Through the edge from this orb, client p99 is about 2.7 s with no load
(`am1-fix.md`). So the driver reports the leader's own `commitMs` beside the client latency, as LF-12 did.

## Design contradictions, quoted

1. **`check_archived` is stricter than the design.**
   - `design/DESIGN.md`, Shape, "Packs": "`Archived` follows only once `Promoted` covers the range, and a range that
     names no pack needs none."
   - `crates/git/src/repo.rs`, `check_archived`, refuses every `Archived` past the last `Promoted`, packs or none.
   - The alarm therefore promotes an empty pack for a range that names none. This costs one shard call and one stored
     pack per such range. It does not change the design.
2. **LF-13's fault cannot leave a gap.**
   - The row (BUILD-PLAN, AM1-WP4): "its AppendEntries dropped for 5 s so the next one leaves a gap".
   - `crates/raft/tests/lf13.rs` shows that under one leader the next round always starts at the follower's `matched`.
     A gap needs a leadership change while the follower is behind, or a truncated log (`InstallSnapshot`).
   - The hosted drivers run both. The row's wording should name one of them.
3. **LF-4 hosted can reach only two of the eight trust resources.**
   - The row asks for "one payload per `HIDDEN_CHARS` range in each `graftyard_trust` resource".
   - The edge serves REST reads of issues and releases only. Pull requests, comments, reviews, review comments,
     commits and check runs have no REST GET on the edge yet.
4. **Session-ref listing needs a client that sends `ref-prefix`.**
   - `design/AGENT-LAYER.md`, Shape: `graftyard-workspace` "reads by v2 `ls-refs`" the current tip.
   - Measured with git 2.56: `git ls-remote <url> refs/graftyard/sessions/x` sends no `ref-prefix`, so it lists
     nothing. `git fetch <url> refs/graftyard/sessions/x:...` does send one.
   - The controller must use fetch, or a raw `ls-refs`.

## Assumptions

- `ARCHIVE_RANGE_MAX` is 2 MiB [guess]. An empty range still promotes an empty pack (contradiction 1).
- The shard's `uploadpack.hideRefs` is the filter. The edge is unchanged, because the edge's v2 advertisement is
  static and every listing comes from the container.
- In the A1 mix, check runs and `RequiredCheck` (no edge route) are replaced by task edits at their rate.
- The run costs are upper bounds at overage rates, with no allowance credited.

## Check results

These ran on `8e1bf69`, and all of them passed.

| Check | Result |
| --- | --- |
| `cargo fmt --all --check` | ok |
| `cargo clippy --workspace --all-targets -- -D warnings` | ok |
| wasm32 clippy: edge, edge `test-faults`, site, site `test-faults`, ops | ok, all five |
| `cargo test --workspace` | 68 suites: 591 passed, 0 failed, 3 ignored (the long liveness and sweep runs that need seeds) |
| `go test ./...` in `containers/git/server` | ok |
| `check_design.py` | OK (agent block and cost figures included) |
| `check_names.py` | OK, 1,807 files |
| `check_links.py` | OK, 2,829 citations |
| `validate.py`, API crate replies | 52 GraphQL, 75 REST, 11 LFS: 0 failed |
| Token scan of every changed file and of the run logs | 0 matches |
