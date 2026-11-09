# AM1 hosted half and the AM1-WP6 gate (LM-1) on graft-m1

These results are from 2026-11-08 and 9, on branch `am1-hosted` (worktree `.worktrees/am1-hosted`), cut from `main` at `9fb5032`. The test install is `graft-m1` (sites a to d, edge `graft-m1-edge`, ops `graft-m1-ops`). Nothing was pushed or merged. `crates/raft` is unchanged, so the long sweep and Porcupine were not rerun. No new record format was taken: AM1's 32.8, 32.12, 32.14 and Authz 0x12.3 cover everything here.

The raw logs and JSONL rows are in `tests/am1/raw/` on the orb. They are git-ignored (`tests/am1/.gitignore`), so this file carries the numbers. The drivers are in `tests/am1/hosted/`.

## Deployed versions

The site days use `workers/site/deploy.mjs` through `tests/am1/hosted/siteday.sh`, with test faults, `GRAFTYARD_AUDIT=1`, and the genesis stand-in. Each edge deploy (`deploy-edge.sh`) and ops deploy (`deploy-ops.sh`) went out right after the site day it belongs to. They are separate scripts, not site-script versions. An edge or ops version does not reset site members.

| Day | Commit | Sites a / b / c / d (member scripts) | Edge | Ops | Notes |
| --- | --- | --- | --- | --- | --- |
| 2 | `40708cb` | `be79725f` / `aed638b3` / `232474cc` / `150bab9c` | `19e72fe6` | `1e9eb629` (the `task` claim) | New shard image `sha256:5db19b4e…`. `--roll-shards` stopped on a catchup timeout, and `tests/am1/hosted/roll.mjs` finished the roll: 58 instances, about 12 catchups answered 500 or timed out (mostly site b's wp6 test repositories) |
| 3 | `a4dd646` | `c2f4c5e4` / `bde58f9a` / `6fe455bf` / `69beff51` | `98de7bf4` | — | `noroll` |
| 4 | `f925d40` | `ed0bff49` / `8686876c` / `11ff63a5` / `f47c2827` | `b330c05c` | — | `noroll` |
| 5 | `c836d14` | `b3d42554` / `7bed34dc` / `e57c7482` / `0b7bcdf4` | `d3a8981f` | — | `noroll`. L0-16's two fixes (below) |
| 6 | `04e4f22` | `6553b6dc` / `751b0701` / `98221437` / `002551dc` | `ce2e1ac3` | — | `noroll`. `origin/main` through `5d18dda` merged (`9532ae8`), including `1c47d28`'s read gate |

`deploy.mjs` uploads each site script twice in a site day. The table lists the second, current version of each, in order a / b / c / d.

The edge's `FORMATS_ENABLED` is its earlier list plus AM1's formats: `32.1,32.4,32.5,32.6,32.7,32.9,16.2,17.2,18.2,20.1,32.10,18.4,32.8,32.12,32.14,18.3`. Here 18.3 is Authz 0x12.3.

## What the hosted half built

Each piece AM1-WP2, WP3 and WP4 listed as missing hosted:

- **Provenance and `changed` on agent pushes and REST ref writes, and `agent::link`** (`ab646f6`, `7a585b6`, `a2270be`, `3dbb4b8`).
  - The git cache shard's `/validate` returns a detail per ref update: new commits with their raw headers and notes, changed paths, fast-forward, and merge base (`containers/git/server/main.go`, `detail_test.go`).
  - The edge asks for detail only for an `agentish` push (`workers/edge/src/agents.rs`). That means an App token once 32.8 is enabled, or an update under a `confine` prefix or `refs/heads/graftyard/`. A default push's `validate` call and its bytes are unchanged.
  - `fill` links the `Graftyard-Session` trailers against the task rows, keeps `changed` and `protected` only for agent updates, and sets `leases` once 32.14 is enabled.
  - `POST /git/refs` and `PATCH /git/refs/{ref}` run the same path on an empty pack, with GHES's fast-forward rule (`may_write`).
- **Minted tokens carry the `task` claim** (ops `1e9eb629`). `graftyard_task` is checked against the task row (`graftyard_git::task::token_task`). It names only a live `GraftyardRun` task.
- **Feed serving on followers and learners** (`2a3c87a`, `197fbba`, `7b06d0f`, `a4dd646`).
  - Each member keeps a ring of fed entries (`workers/site/src/feed.rs`, `graftyard_git::feed::Ring`), holds polls under FEED_POLLS (2 per token, 50 per IP) and FEED_HELD_MAX (1,000), and answers held polls at once when it stops following.
  - The edge routes with `feed::route`. It leaves out the voter whose `/status` says it leads, and drops its cache on `notServing`.
  - `/status` reports the peak held count, refusals, polls, and leader polls. `/learners {"feed": true}` places learners from the voters' peaks and refusals (`learners_for_feed`).
- **`stalled_since` wiring in the Durable Object.** The member's `stalled_since` is on `/status`. The next write after a stall carries `extend` (AM1-WP4's raft code). It is exercised by L0-12's stall below.
- **The PR body-edit endpoint for plan blocks.** GraphQL `updatePullRequest` with `body` (`ab646f6`). LF-5 below.
- **AM1-WP1 and AM1-WP5.** AM1-WP1's admission classes ride the same `provenance`, and the edge passes it to `admission`. AM1-WP5's trust view needed nothing new hosted for the rows run here. The hosted LF-4 run did not happen (see "Not run").

Fixes the hosted runs found, each verified after a redeploy:

1. `/feed` panicked: a `RefCell` was borrowed inside an `if let` (`feed.rs`), which reset the isolate (`197fbba`).
2. An idle follower read as lagging, so every poll got 429. Feed lag was measured against the log's last entry and the ring. It now runs from the member's applied entry to the newest committed entry it heard of (`7b06d0f`).
3. The pending ring buffer is now bounded to FEED_RING (`197fbba`).
4. L0-16 (day 5, `13b0a47`): the edge read both tiers before any lease command and answered 503 "the repository's metadata tier is loading" while the metadata tier was unloaded. The lease route and the list now read through `read_core`, which serves an empty metadata view while that tier loads. `decide` already decided lease commands on the core tier alone (AM1-WP4).
5. L0-16's harness: a held metadata tier was being replayed by the alarm's `load_meta`, which counted that replay's reads as path reads. A held tier is now skipped by `load_meta` until release (test faults only).

Setup change, not a bug: m3-ci (App 1114112) lacked `issues: write`. `tests/am1/hosted/app-perms.mjs` gave the App and installation 7190001 contents, issues, pull_requests and checks write, plus metadata read.

## Main's read gate (`1c47d28`)

`origin/main` (through `5d18dda`) was merged into `am1-hosted` before site day 6 (`9532ae8`). The conflicts were in `workers/edge/src/agents.rs` and `api.rs`. Main replaced `read_named` with `read` (which now runs `authorize_read`), and this branch had added `read_core` for the lease route (L0-16). `read_core` now passes `Gate::Readers` to `read_repo`, so the lease routes go through the same gate, and the feed reads through `read`. After the merge: fmt, workspace clippy, the five wasm32 clippy runs, `cargo test --workspace` (565 passed, 0 failed), the three scripts and `go test` all pass, and the plants are 25 of 25 caught (`raw/plant-run3.log`).

Under the gate on day 6 (`m3org/j4`, private):

- m3-admin, m3-member and m3-ci (`ghs_`) each get 200 on a task read, the lease list and a feed poll.
- An anonymous caller gets 404 on the task read and 401 on the lease list and the feed. The gate answers 401 for an anonymous request before the route runs.
- An App acquire gets 201 and its release 204, and an App clone works.
- An anonymous `git ls-remote` gets 401 ("Authentication required").
- L0-16 reran on day 6 with the same result: 0 metadata reads on 3 members for 8 checks.
- LF-3 reran: 410 with `oldest_cursor` (851 ms), `wait=40` held 26.1 s, 33 paths 422, 32 paths 200.

Nothing in the AM1 hosted runs broke under the gate. The stall, B0-36, B0-37 and L0-12 ran on day 5, before the merge. They were not rerun.

## LM-1 rows

| Row | What ran hosted | Pass line | Result |
| --- | --- | --- | --- |
| Base journeys | j2 (PR flow) and j3 (issues and labels) 20 rounds each; journey 4 / BM-3 on `m3org/j4` with m3-ci unflagged | Unchanged | Pass. j2 20/20, j3 20/20. Journey 4: PAT pushes 10/10, revokes 204 ×10, refused after revoke 10/10. `ghs_` pushes 10/10, `jti` revokes 204 ×10, refused 10/10. Member removal: 403, nothing moved. A first journey 4 run with m3-ci flagged had its `ghs_` pushes to `main` refused by `confine`, as designed (kept as `raw/j4-flagged`) |
| L0-11 (byte diff) | `bytediff.py`: 128 distinct read-only REST and GraphQL requests an unmodified client sends (gh's documents and REST calls), 8 rounds, against `graft-m1-edge` before AM1 (`main-be148a05`) and after | Identical except ids and times | Pass. Day 2 edge `19e72fe6`: 2,048 replies, 0 differ. Day 5 edge `d3a8981f`: 1,024 replies, 32 differ, all in 4 requests (`wp6j` issue lists over REST and GraphQL, logged in and anonymous). Status and kept headers are identical in all 32. The bodies differ because the base journeys' j3 rounds created issues 69 to 88 in `wp6/wp6j` between the baseline and this run, and the list's first page now shows them. The day 2 run and the day 5 run differ in the same 4 requests and in no other. The diff is data, not the layer. A clean rerun needs a fresh baseline on main's edge, which would need main's edge deployed again, so it was not rerun. Day 6 edge `ce2e1ac3` (main's read gate merged): 2,048 replies, 48 differ. 32 are the same `wp6j` issue-list data change as on day 5. The other 16 are `wp6j-issues-anon`, `wp3m-issues-anon` and `main-issues-anon`, 200 on main's pre-gate edge and 404 now. The three `wp6` repositories are private, and an anonymous read of the repository itself was already 404 on both edges. So the anonymous issue list was the leak `1c47d28` closes. It is the read gate's change from main, not the agent layer's. API reply dump (`GRAFTYARD_REPLIES`, `cargo test -p graftyard-api`) on this branch against `main` `9fb5032`: 120 of 120 files identical |
| L0-12, refusal counts | `rows.mjs l012`: an Exclusive lease on `l012/<tag>/**` held by m3-member through an external task, under `AgentsExclusive`, on j4. For each actor kind, 100 pushes and 100 REST ref writes touching the scope | 100 of 100 refused for each `ActorKind::Agent` kind, 0 of 100 for the others | Pass for the kinds the install can field. Confined vendor App: pushes 100/100 and writes 100/100 refused, each naming the lease ("l012/…/** is leased Exclusive by user 7100002 (task …)"). Flagged App without `confine`: 0/100 pushes refused, 0/100 writes refused by a lease (99 accepted, 1 answered 422 "Object does not exist" by the git cache, a lost race between the scratch push and the write). Unflagged App: 0/100 and 0/100. Human PAT: 0/100 and 0/100. Graftyard-run App and `Graftyard MCP`: not run (no such Apps registered) |
| L0-12, stall | `stall.mjs 30 20` on j4 (run 2): 20 leases with a TTL of 3,600 s (10 Exclusive on distinct tasks, 10 Shared). Then every member call on every site failing (`/_bed/fault?site=255`) for 30 minutes with every member running. Task writes went out every 250 ms across the cutover, so the leader held an entry in flight. Then the first write, the leases read back, member clock offsets, and an idle 60 s Exclusive followed 10 minutes later by an overlapping Exclusive from another task | Every lease live after with `expires` moved by the recorded stall; clock offsets under 500 ms; the late acquire succeeds | Fail on the `expires` line; the other measured lines pass. The stall lasted 1,814,090 ms. The leader (site 0, term 2) recorded `stalledSince` = 1794180678283 at its in-flight entry 4327 and held it, still leader, for all 30 minutes. The followers recorded none: `suspect` reports are member calls, which the fault blocked. When the fault cleared, site 2 won term 3 and appended a noop (4328) with no `extend`. Applying 4328 (`at` after the marker) cleared site 0's marker. After the stall: 20 of 20 leases live, but `expires` moved by 0 for all 20, so `expires − granted` = 3,600 s exactly. They are live only because 3,600 s outlasts 30 minutes. Member clocks against each other: spread at most 114 ms before and 61 ms after, with half round trips at most 156 ms. The absolute offsets of about 5.7 s are this orb's clock, which is not NTP-synchronized and reads 6 s behind cloudflare.com's `Date`. The late overlapping Exclusive got 201 with 0 conflicts. Cutover writes: 3 × 201, 15 × 500, 6 × 503. Run 1, which sent one write at once and so reached no `decide`, had the leader step down at minute 2 with no marker on any member, and was stopped. See contradiction 6. No two holders held overlapping Exclusive leases (no overlapping holders were set up, so `DRIFT` was not measured) |
| L0-12, A1 replay and feed latency | — | — | Not run: no hosted A1 replay driver at 500 agents (see "Not run") |
| L0-13 | `rows.mjs l013` on j4 through the edge, 100 of each | 100 of 100 refused for each bad acquire | Pass for what ran. Exclusive `**`, `*`, `*/x`, a user token with no live task, a second Exclusive for one task, an App token with no session: 100/100 each, all 422. The first Exclusive for the task 201, its release 204. LF-1 covers the uninstall. Not run: the 600 + 600 plan blocks (needs 120 accounts without push access), the holder losing push access, the 5 h renew |
| L0-15, feed half | `feed.mjs l015` | Feed polls get 401, 403, or 429; the leader sees zero feed requests | Pass with a status difference. Anonymous 401. A `metadata: read`-only token 404, not 403 (see "Contradictions"). Over FEED_POLLS 429 with `Retry-After: 10`. `leaderPolls` 0 on every member. Not run: the `AgentGate`, Code Mode, and `/log` and prompt halves (no MCP server or controller hosted) |
| L0-16 | `rows.mjs l016` on lf1-2, day 5: every member of the group held its metadata tier unloaded and counted every read of it (`/bench_seed {"metaUnload"}`). Then a human push, a mint, an agent push outside leases, an agent push into another holder's live Exclusive (refused), a Shared acquire, a conflicting Exclusive acquire (409), a renew, a release, and a REST merge. The reads were counted after each | Every check passes or refuses with zero metadata loads | Pass for pushes, minting and lease checks: 0 metadata reads on all three members for each of the 8 checks. The push into the Exclusive was refused with the lease's text, Shared 201, conflicting Exclusive 409, renew 200, release 204. The merge is 503 "metadata tier is loading" by design: `MergeCmd::Merge` writes `Merged` to the metadata tier (`tiers()` is both), and the edge plans it from the pull row, a metadata row. It merged (200) once the tier was released. Before day 5 the same run read 6 to 9 metadata rows per push (the alarm's replay, fix 5), and the lease commands answered 503 (fix 4) |
| LF-1 | `rows.mjs lf1`: 100 external tasks across 10 repositories (j4, lf1-0..8), then the operator's `Uninstall` (the edge serves no uninstall route), reads, the next command, and a reinstall | Every task reads `cancelled` at the next read with no sweep, and its next command records it | Pass. 100 `queued`, then 100 `cancelled` at the first read (9.8 s for all 100 reads). The leader logs show only `TaskCreated` and `TaskUpdated`, so no read wrote an event. Next command (admin cancel): 409 `cancelled` ×100. After reinstall they stay cancelled |
| LF-2 | `DELETE /api/graftyard/v1/agents/apps/1114112` (204), then m3-ci pushes to `main` | The next decide no longer classes it Agent | Pass. Refused while flagged; accepted on the first push after the 204 |
| LF-3 | `feed.mjs lf3` | 410 with `oldest_cursor`; `wait` clamped to 25; 422 for 33 paths | Pass. Foreign cursor 410 with `oldest_cursor` (669 ms). `wait=40` held 25.9 s. 33 paths 422; 32 paths 200 |
| LF-5 | `rows.mjs lf5`: five draft PRs through GraphQL `createPullRequest`, then a body edit through `updatePullRequest` | One lease from the valid block, none from the others, bodies unchanged | Pass. Valid block 1 lease. Repeated key, bad glob, second block, over 4 KiB: 0 each. All five bodies unchanged. The `updatePullRequest` body edit made 1 lease, body unchanged |
| LF-12 | `feed.mjs lf12` and `lf12c` on lf1-0. Phase A through the edge from one IP; phase B 5,000 polls straight to the followers with 50 commits; `place_learners` (`/learners {"feed": true}`); phase C 5,000 polls across the learners with 50 commits | Commit p99 ≤ 1 s before placement and ≤ 600 ms after; zero `overloaded`; ≤ 1,000 held per member, the rest 429 with `Retry-After`; zero polls on the leader; zero on followers after placement | Fail on commit latency; every other measured line passes. A: FEED_POLLS' 50 per IP binds, 40 × 200 and 360 × 429, each 429 with `Retry-After`. B: each follower held exactly 1,000, the rest `full`; leader polls 0; client commit p50 1,333 / p99 2,093 ms. `place_learners` added 11 learners (config index 73) from the followers' peaks (1,000 each) and refusals (1,860 and 1,476). C: 5,000 held across 11 learners (454 or 455 each); voter polls 0 before and after; client commit p99 2,317 ms. The leader's member `commitMs`: 106 samples, p50 442, p99 3,414, max 3,506 ms, with the misses clustered in the hold bursts. Not measured: `overloaded` AppendEntries replies |
| LF-13 | `feed.mjs lf13` on lf1-1: 1,000 polls held on follower 1, then its site's member calls failing for 5 s while 16 commits ran | Every held poll returns within 1 s with its cursor and no events, new polls refused until caught up, no event lost | Not shown hosted. The fault did not make the follower stop following: the leader re-sent its AppendEntries inside the 5 s (COMMIT_RETRIES), so no reply carried a gap, and no `InstallSnapshot` ran. The 1,000 held polls returned at their 25 s wait (p50 25,274 ms), not at once. 20 probe polls while the fault was on answered `full` (the member held 1,000), not `lagging`. No event was lost (16 commits 200). Forcing a gap needs a fault that drops entries rather than failing calls, or log compaction past the follower; neither exists on the install. The pure model passes (`tests/am1/results/am1-wp4.md`) |
| B0-29 | — | — | Not run hosted: the 500-agent journey 9 replay needs a load driver this branch does not have |
| B0-30 | Plan and cost only | Member SQLite under `CHILD_GROUP_AT` with zero `SQLITE_FULL`; archived entries readable | Not run (see "B0-30 plan") |
| B0-31 to B0-35 | — | — | Not run hosted: replay-scale load rows with no driver here |
| B0-36 | `rows.mjs b036` on j4 for the confined vendor App (m3-ci flagged `third_party`, `confine refs/heads/claude/**`), 100 of each action over every surface the edge serves | 100 of 100 refused for each, both agent kinds; the others never refused | Pass for the confined vendor App, on every surface the edge serves; Graftyard-run kind not run. 100 of 100 refused for each: approve (GraphQL `addPullRequestReview` APPROVE, "agents may not approve pull requests"); merge over REST `PUT pulls/{n}/merge` (403 "Resource not accessible by integration") and GraphQL `mergePullRequest` (403); `enablePullRequestAutoMerge` ("Auto merge is not allowed for this repository"); release (REST `POST releases`, 403 "agents may not create releases or tags"); tag push over git ("agents may not create releases or tags"); a `graftyard/` status (REST `POST statuses`, 401: the edge serves status writes only to user tokens); a push outside its prefix over git and a REST ref write outside it (`POST git/refs`), both "app 1114112 may push only refs/heads/claude/**"; a protected path in its prefix over git (`.github/workflows/`, "is a protected path"); a rule or settings edit (`PUT agents/policy`, 403 "agents may not edit rulesets ..."). REST `POST git/refs` with a `CODEOWNERS` change in its prefix: 99 refused "CODEOWNERS is a protected path", and 1 answered 422 "Object does not exist" by the git cache (the same scratch-push race as in L0-12), so 100 of 100 were not accepted. Controls: an ordinary push in its prefix 100/100 accepted, a draft PR through GraphQL 100/100 accepted. No surface on this edge: ready for review (no `draft: false` write) and labels (no `issues/{n}/labels` write). The merge refusals come from the edge's App gate before `decide` (`commit_merge` answers 403 for any App), so they would also refuse an unflagged App. The second half (a flagged App without `confine` and a user token pushing into an Exclusive scope) is L0-12 above. The `Co-authored-by` push is in B0-37 below |
| B0-37, push half | `rows.mjs b037` on lf1-3: 100 pushes per kind with malformed trailers (bad URL, control characters, a 4 KiB line, a repeated key, no blank line), forged `Graftyard-Session` trailers (another App's id, an unknown task, another repository's origin), a linked control, 100 writer pushes to `refs/graftyard/`, and a user push whose only trailer is `Co-authored-by` | Zero pushes fail on parsing; every forged trailer stays `Claimed`; every `refs/graftyard/` update not by `graftyard-workspace` refused | Pass for the push half. Zero pushes failed on parsing: 900 of 900 trailer pushes accepted, except 2 whose push answered with no refusal text (1 in "repeated key", 1 in "unknown task"; transport, not parsing; the leader's log has 99 provenance rows for each of those kinds). Every forged and malformed trailer is stored `Claimed`: bad URL 100, control characters 100, a 4 KiB line 100, no blank line (no trailer block, so no provenance row), another App's task 100, an unknown task 100, another repository's origin 100. In the repeated-key commits the second value is `Claimed` beside the first, linked one: 99. The linked control is `Session` 100 of 100. Writer pushes to `refs/graftyard/` by m3-admin: 100 of 100 refused ("deny updating a hidden ref"). The user push whose only trailer is `Co-authored-by` was accepted with no provenance row, so it stays Human (`classes` is false). Scaled: 100 per kind (1,000 pushes), not 10,000 |
| B0-37, fetch half | — | Session refs appear only to a v2 `ref-prefix` that starts with them | Not run: the edge does not advertise session refs to a v2 `ref-prefix` (no `graftyard-workspace` App, no session refs) |

## B0-30 plan and cost (not run)

B0-30 is 30 days of the journey 9 replay on a group seeded with the 4 GB metadata tier. The plan: 3 members seeded through `/bench_seed {"load"}` with 4 GB, then journey 9's command mix.

- About 54.9M rows written: about $55 in Durable Object writes.
- Durable Object storage 25.3 GB-month: $5.07.
- R2 archive 152 GB-month: $2.28.
- Durable Object requests: $2.06.
- Total about $65.
- Wall time: about 40 h at 3 entries/s with one entry in flight, or 30 days in real time.

It cannot pass hosted today: there is no hosted archive alarm (`archive_due` is not wired), so the log would not archive to R2 and "archived entries readable for `RestoredTo`" has nothing to read. Run it once the archive alarm is hosted.

## Plants

`tests/am1/hosted/plant.py` plants 25 mutants, one at a time, in the code this round added or changed, and restores each file byte for byte. 25 of 25 were caught (`raw/plant-run2.log`):

| # | Mutant | Caught by |
| --- | --- | --- |
| 0 | every push asks for detail | `agents::tests::only_agentish_pushes_ask_for_detail` |
| 1 | a user push under a `confine` prefix asks no detail | same |
| 2 | detail asked before 32.8 is enabled | same |
| 3 | the token claim is not among the sessions linked | `agents::tests::fill_links_the_claim_and_keeps_paths_only_for_agent_updates` |
| 4 | `changed` kept for a user push outside every agent prefix | same |
| 5 | `protected` dropped for an agent update | same |
| 6 | `leases` set before 32.14 | same |
| 7 | a non-fast-forward PATCH accepted | `agents::tests::a_rest_ref_write_needs_a_fast_forward_unless_forced_or_created` |
| 8 | `force` ignored on a REST ref write | same |
| 9 | detail lines sent for a default push | `the_git_cache_s_detail_links_through_the_edge_s_path` |
| 10 | detail lines name the old tip as the new | same |
| 11 | `validated_of` drops protected paths | same |
| 12 | `validated_of` ignores the commits (no provenance) | same |
| 13 | the ring serves events at or before the cursor | `the_ring_a_serving_member_keeps_matches_the_pure_feed` |
| 14 | the ring ignores the `paths` filter | same |
| 15 | `learners_for_feed` ignores refusals | same |
| 16 | `updatePullRequest` takes both body and title | `surfaces` |
| 17 | `updatePullRequest` ignores an unserved field (`state`) | `surfaces` |
| 18 | a core-only read refused while the metadata tier loads (L0-16) | `agents::tests::a_core_only_read_is_served_while_the_metadata_tier_loads` |
| 19 | every read served without the metadata tier | same |
| 20–24 | Go: commits reachable from existing refs reported as new; `ff` always true; merge-base paths lost; a delete gets detail; an update line without `refs/` accepted | `containers/git/server` `go test` |

## Commits

On `am1-hosted`, oldest first: `1138608` (site day and edge deploy scripts), `7a585b6` (git cache `/validate` detail), `ab646f6` (edge provenance, `changed`, linking, REST ref writes, `updatePullRequest`), `2a3c87a` (feed serving on followers and learners), `40708cb` (site day 2, ops deploy), `197fbba` (feed panic, `Following`, bounded pending, `/log` provenance), `a4dd646` (every-site fault, `/learners` from feed peaks, metadata hold, poll counts, edge skips the leading voter), `7b06d0f` (feed lag), `f925d40` (released metadata tier reloads), `a2270be` (`agentish`/`fill` split with tests), `3dbb4b8` (`may_write`), `fd9ac79` (plants, drivers), `35807e6` (LF-5 driver), `85d83d3` (l012 per kind), `13b0a47` (L0-16 fixes, b036 and l016 drivers, site day 5 script, 2 plants), `c836d14` (b037 driver), `8876b02` (stall driver writes across the cutover), `9532ae8` (merge of `origin/main` with the read gate), `04e4f22` (site day 6 script), and this file.

## Checks

On the branch head after the merge with main (`9532ae8`):

- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- wasm32 clippy, `-D warnings`: site and edge each with and without `--features test-faults`, and ops: all clean.
- `cargo test --workspace`: 565 passed, 0 failed (546 before the merge).
- `python3 scripts/check_design.py`, `check_names.py`, `check_links.py`: pass.
- `go test ./...` in `containers/git/server`: ok.
- Token scan of `tests/am1/raw` and `/tmp/am1` (852 files, after day 6): 0 matches for `gh[pousr]_…`, `github_pat_…`, or a JWT. 0 hits for the 8 known secret values (the bed and m0 tokens, the m3 PATs, and the wp6 tokens). Tokens reach git only through a credential helper reading a 0600 file, and reach `curl` and `fetch` only in headers.

## Not run, and what the hosted half still lacks

- No `graftyard-workspace`, `GraftyardRun`, or `Graftyard MCP` App is registered on graft-m1. So these did not run: L0-12's Graftyard-run and MCP kinds, B0-36's Graftyard-run kind (including a push with no live task and a fork push after cancel), the `task` claim reaching `decide` through a minted token, and B0-37's `refs/graftyard/` pushes by `graftyard-workspace`.
- The edge does not advertise session refs to a v2 `ref-prefix`, so B0-37's fetch half cannot run.
- No hosted archive alarm (`archive_due`), so B0-30 cannot pass.
- No hosted A1 / journey 9 load driver, so B0-29, B0-31 to B0-35, and L0-12's replay and 500-poller latency lines did not run.
- LF-13 needs a fault that leaves a gap or forces an `InstallSnapshot` on a live follower.
- LF-4 hosted (trust view over the edge with real payloads) did not run.
- L0-13's plan-block half (600 + 600), the holder losing push access, and the 5 h renew did not run.
- L0-15's gate, Code Mode, and `/log` and prompt halves need an MCP server and the controller.
- The edge serves no ready-for-review or `issues/{n}/labels` write, so B0-36 has no surface for those two actions on this install.

## Contradictions with the design

1. **Feed status for a `metadata: read`-only token.** `build/BUILD-PLAN.md`, L0-15: "Feed polls get 401, 403, or 429". The edge answers a reader without metadata access 404, the same answer the task and lease reads give a reader who cannot see the repository (`crates/api/src/tasks.rs`, `Reader`). Either the row should allow 404, or the feed should answer 403 when the repository is visible.
2. **`graftyard_task` only for Graftyard-run tasks.** `design/AGENT-LAYER.md`, Shape: "A token request's additive `graftyard_task` field must name a live `GraftyardRun` task." An external task's App cannot bind a token to its own task. So a vendor App's push links only through a trailer, and its leases need the session header on every call. This is not a code bug, but it means no hosted row can exercise the claim path without a registered `GraftyardRun` App.
3. **Lease "another holder" for a pusher with no task.** `design/AGENT-LAYER.md`, `LeaseEnforce`: "Another holder's lease is one whose `session` differs from the pusher's task, or, for a pusher with no task, whose actor differs." A confined vendor App with external tasks pushes with no task (its `a.task` is set only from a claim, which external tasks cannot have, see 2). Every lease it holds through any of its tasks is therefore "its own", so one task's Exclusive never binds the same App's other tasks. The first L0-12 run, with the holder's lease taken by m3-ci itself, accepted 100 of 100 conflicting pushes for that reason. The hosted run uses m3-member as the holder.
4. **LF-12's commit latency.** `build/BUILD-PLAN.md`, LF-12: "Commit p99 at most 600 ms once `place_learners` has run and at most 1 s before it". Hosted, with real held polls, the leader's member commit p99 was 3.4 s, clustered in the hold bursts. AM1-WP6 says the row reruns "on real code". The M0 measurement had 1 pass and 3 between the lines in 4 runs, and this run is in the fail band.
5. **L0-16's "merges".** `build/BUILD-PLAN.md`, L0-16: "Pushes, merges, minting, and lease checks on the A1 replay with the metadata tier unloaded". A merge writes `Merged`, which `tier_of` puts in the metadata tier, and it plans from the pull row in the metadata tier, so it cannot run with that tier unloaded. The design's Shape agent block says "Core rows are what pushes, merges, and minting read". The merge's reads of the pull row and its `Merged` write contradict that.
6. **The recorded stall is lost when leadership moves.** `design/DESIGN.md`, Shape, `ExtendLeases`: "A member marks `stalled_since` at the earliest of a follower's first `suspect` report, a candidate's campaign start, or the leader's first entry that no quorum acknowledged within CALL_DEADLINE ... The next entry the group commits carries `ExtendLeases { since, by: at - since }`". And L0-12: "Every lease live before the stall is live after it, with `expires` moved by the recorded stall". With every member call blackholed, no `suspect` report arrives and no campaign starts until the calls work again. The only marker is the old leader's. When another member wins the next term, its campaign marker is about the time the fault cleared, so `by` is about zero and no `extend` is written. Its first entry, stamped after the old marker, then clears the old leader's marker (`member/log.rs`: "The marker clears when an entry carrying `extend` commits, or one stamped after it"). Measured hosted: 30.2 minutes of stall, 0 ms of extension on 20 leases. Either the marker has to travel to the next leader (in the vote or the first AppendEntries reply), or a new leader has to take the earliest marker a quorum reports. This touches `crates/raft`, so it was not changed here.
