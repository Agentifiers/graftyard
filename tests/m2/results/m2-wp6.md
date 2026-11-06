# M2-WP6: the Milestone 2 gate (BM-2) on `graft-m1-*`

**Verdict: BM-2 fails on its latency clause and passes the other two.** Journeys 2 and 3 pass (10 of 10 each,
every reply valid against GHES 3.22). A queue entry with a stale `parent_tip` is refused 100 of 100 on the final build.
The merge p99 on a freshly elected leader is **3.20 s** (n = 91, p50 2.28 s), over the 2 s gate. Only enam-elected
leaders come near it (p99 2.09 s); weur and apac leaders are over 2 s in every sample. The cause is cross-region round
trips that remain after the serial steps were removed ("Merge latency" below). It is not a fault one more fix removes.

| Clause | Result | Numbers |
| --- | --- | --- |
| Journey 2 (`gh pr`: review, required check, merge, view, list) | **Pass** | `j2a` 1/1, `j2b` 10/10; merge refused until review and check commit, then MERGED with parents (base, head) |
| Journey 3 (`gh label`, `gh issue create -l`, `gh issue list`, `gh api .../issues`) | **Pass** | `j3a` 2/2, `j3b` 10/10 |
| Merge p99 on a freshly elected leader ≤ 2 s | **Fail** | p99 3.20 s, p50 2.28 s, n = 91 on 91 distinct new terms; enam 2.09 s, weur 3.20 s, apac 3.16 s |
| Stale `parent_tip` refused 100/100 | **Pass** | `bm2` 100/100 (sd7 build) and `bm2f` 100/100 (final sd10 build): each trial `StaleChain` ×3, queue unchanged |


Runs on 2026-11-06, against the test install `graft-m1-site-a/b/c` and `graft-m1-edge` (test-faults builds), on branch
`m2-wp6` from `main` `64530ea`. Clients: gh 2.102.1 and git with the install's writer token (`wp6-writer`, account
7000002). Raw data in `raw/m2-wp6/` (gitignored).

## Merge latency

The first measurement, on main's build with the leader stable, was far over the gate. gh's debug log put a whole
`gh pr merge` at about 12.5 s: `PullRequestByNumber` 3.06 s and `mergePullRequest` 9.16 s. A test-faults-only
`server-timing` header on the edge's API replies, a `x-graft-trace` of the commit's hedge steps, the leader's
`/stats` and `/deps_log`, and the shard's catch-up step times located it. Each cause, with its fix and its test:

| Cause | Cost | Fix | Test (fails on the old code) |
| --- | --- | --- | --- |
| The lookup rule read every record site's `MemberRegistry` row, one after another, before it called any member. Every edge read and every member's dependency read paid three cross-region registry calls first | About 400 ms per lookup; 2.4 s of a 3 s GraphQL read (auth, `Names` login, `Names` placement, `Authz`, rows) | `crates/lookup`: a pass reads a stage only once every earlier candidate missed: the cached config and the record's bootstrap names, then that record's registry rows (in parallel), then `moving_from` | `a_lookup_reads_registry_rows_only_after_the_names_missed` |
| The leader read an entry's three dependencies (`Names`, `Tokens`, `Authz`) one after another, though "The write path" reads them "once through `fresh`, in parallel" | The sum of three reads, 1.0 to 1.6 s before the lookup fix | `workers/site/src/host.rs`: `each_at_once` | `dependency_reads_run_at_once` |
| The shard's catch-up read the chain's first index, then its refs, from the leader, one after another | Two leader round trips; 300 to 500 ms once leadership moved off the shard's site | `crates/git/src/ingest.rs`: `heads` reads both for every chain at once | `a_catch_up_reads_first_index_and_refs_at_once` |
| The edge read the repository's tiers, then the owner's `Authz`; for a node-id target it resolved the repository by id, then again by name; and it probed the group (`ensure_group`) before every commit | Two to three extra member reads per mutation | `workers/edge/src/api.rs`: tiers and `Authz` at once, one resolve, no create probe after a leader read | (edge-only wiring; measured below) |
| The shard re-read the owner's `RepoOwner` and `Placement` rows on `Names` for each merge plan | Two `Names` reads, about 400 ms | The edge passes the owner's record it already read (`&record=`) | (wiring; measured below) |
| The shard caught up behind its `IngestCursor` before every merge plan, though `PrSync`'s test merge usually caught it up already | 290 to 610 ms of each plan once leadership moved (the shard's own `x-graft-timing`: catch-up 300/289/602/303/305/613 ms, the container's merge 76 to 84 ms) | `crates/git/src/merge_cache.rs`: `merge_then_catch_up` merges first and catches up only when the container answers 409 (a commit is not there) | `a_merge_catches_up_only_when_a_commit_is_missing` |
| `commit_hedged` looked the leader up with `/outcome_by_request` before its first `/commit` send, even with the leader cached from the read just before | One round trip to the leader, 60 to 310 ms (`x-graft-trace`: first send at 60 to 309 ms) | `crates/lookup`: the first send goes to `cached_leader`; `NotLeader` starts the lookup | `a_commit_starts_at_the_cached_leader` (the helper; the send path is measured) |
| `graphql_post` authenticated, then resolved the repository | One `Tokens` read, 60 to 330 ms | Both at once | (wiring; measured below) |

Stable leader, `wp3m`, after the lookup and dependency fixes: `mergePullRequest` 4.9 s, then 1.6 to 1.7 s.

Fresh leader (`tests/m2/wp6/p99.py`, leadership moved with `/transfer` before each sample):

| Build | n | p50 | max | Notes |
| --- | --- | --- | --- | --- |
| lookup + deps fixes | 6 | 2.36 s | 3.89 s | `try2` |
| + catch-up, edge reads | 9 | 2.40 s | 2.73 s | `try3`; apac leader slowest |
| + record to the shard | 9 | 2.23 s | 3.04 s | `try4` |
| same, first 22 of a 300 run | 22 | 2.44 s | 3.09 s | `pre1`, stopped to fix the next two causes; per new leader site, p99: enam 2.29 s, weur 2.84 s, apac 3.09 s |

Where a fresh-leader merge's time went in `try5` (the apac leader, sample 2, 2.79 s at the client): auth 64, resolve
489, tiers and `Authz` 315, merge plan 759 (of which the shard's catch-up 602 and the container's merge 76), commit
1,088 (the first send at 185). The client is in LAX; the edge runs in LAX; the leader's own `commitMs` is 660 to 820 ms
(one replication round of 170 to 215 ms after three dependency reads of 210 to 380 ms each from apac, where the
`Names`, `Tokens`, and `Authz` leaders are on enam).

## Journeys

`tests/m2/wp6/journeys.py` on `wp6j` (rule on `main`: `ci` required, one approving review).

- Journey 3, `j3a`, 2 of 2: `gh label create bug` 201, again 422 `already_exists` (gh prints "label with name "bug"
  already exists; use `--force`"), `--force` exit 1 (it sends `PATCH /labels/bug`, which is not served).
  `gh issue create -l bug`, then `gh issue list --label bug` and `gh api repos/o/r/issues?sort=updated&labels=bug` at
  once: both include the new issue, the REST list newest-updated first.
- Journey 2, `j2a`, 1 of 1: `gh pr merge` refused while unreviewed and unchecked; then `gh pr review --approve`,
  `POST /statuses` `ci` success, and `gh pr merge --merge` at once. Two merges were refused (decided before the
  review or the check committed), the third merged at 3.98 s. `gh pr view` (MERGED, APPROVED) and
  `gh pr list --state merged` at once both show it; `main` is the merge commit (base, head).

## Fresh-leader merge p99, the 300-sample run (`p99`)

`tests/m2/wp6/p99.py p99 300 --gh-every 25` on `wp6p`, edge `516c5fd6` and sites `f1bd2761`/`1b448904`/`b6539846`
(every fix above). Before each sample, `/transfer` moves leadership to the next site (0, 1, 2 in turn); the sample
waits for the new leader's term, then sends `mergePullRequest` (`clientMutationId`, as `gh pr merge` sends it) and times
it at the client. 94 samples ran, 06:11 to 06:32 UTC, each on a different term (94 distinct terms, `from != to` in every
sample), election 801 ms p50 and 1,611 ms max. I stopped it at sample 94: the verdict was settled and more samples could not move it.

| New leader | n | p50 | max (= p99 at this n) | Over 2 s |
| --- | --- | --- | --- | --- |
| 0, enam | 30 | 1.72 s | 2.09 s | 2 |
| 1, weur | 31 | 2.28 s | 3.20 s | 31 |
| 2, apac | 30 | 2.51 s | 3.16 s | 30 |
| all | 91 | 2.28 s | 3.20 s (p99 3.20 s) | 63 |

Every sample merged (91 of 91 mutations and 3 of 3 whole `gh pr merge` runs, 2.3 to 3.2 s). There were no retries.

**Why n = 91 is enough for this verdict.** The gate needs p99 at most 2 s, which means at most 1% of samples over 2 s. Here
63 of 91 (69%) are over. Under the null hypothesis "p99 ≤ 2 s", each sample exceeds 2 s with probability at most 0.01,
so 63 or more of 91 has probability below 10^-100. More samples can only tighten a fail this wide. A pass would need
about 300 samples, where p99 is the 297th order statistic with 3 above it. For enam leaders alone, 2 of 30 are over
2 s (2.09 s max), so even that site is at or over the line.

Where the time goes (per new-leader site, p50 / max in ms, from the edge's `server-timing`):

| Site | auth + resolve | tiers + `Authz` | merge plan (shard) | commit |
| --- | --- | --- | --- | --- |
| enam | 312 / 587 | 351 / 627 | 561 / 630 | 407 / 436 |
| weur | 340 / 501 | 469 / 934 | 781 / 1,019 | 612 / 834 |
| apac | 332 / 600 | 591 / 938 | 653 / 757 | 918 / 1,184 |

The client and the edge are in LAX. Every step makes cross-region calls: the identity groups (`Names`, `Tokens`,
`Authz`) lead on enam, the repository's leader is on its new site, and the leader's own commit reads its three
dependencies from enam before one replication round. A weur or apac leader pays two to four more transpacific or
transatlantic round trips than an enam one. No remaining step is serial without reason; each of the four is one
round to one leader, or reads at once.

## Journeys 2 and 3, on the status-id build (sd8: sites `ccdfc76e`/`e7c0e891`/`451228ca`, edge `3a2e4f02`)

- Journey 3, `j3b`, **10 of 10 pass**. Each trial: `gh issue create -l bug`, then `gh issue list --label bug` and
  `gh api repos/o/r/issues?sort=updated&labels=bug` at once. Both include the new issue, and the REST list has it first.
  `gh label create bug` ran in `j3a`: 201, then 422 `already_exists`.
- Journey 2, `j2b`, **10 of 10 pass**. Each trial: `gh pr merge` is refused while the PR is unreviewed and unchecked
  (exit 1). Then the owner's `gh pr review --approve`, CI's `POST /statuses` `ci` success, and the writer's
  `gh pr merge --merge` (retried) run at once. 1 to 3 merges are refused before both facts commit. The merge lands
  2.73 to 4.69 s after the three start. `gh pr view` (MERGED) and `gh pr list --state merged` at once both show it.
  `main` is the merge commit (base, head) in all 10 trials.
- Validation (`gh-replies.py`, then `validate.py` against GHES 3.22): `j2b` 183 GraphQL and 10 REST replies, `j3b` 60 and
  30, **0 failed**. Earlier captures: `j2a` 20/1, `j3a` 12/7, `p99-gh` 16/2, `smoke` 29/4, `try1-gh` 8/1,
  `bf23r` 42/3, `bf22b` 20/6, `bf22d` 20/6, `bf22e` 19/6, all 0 failed.

## BF-22 reduced, and two bugs it found

`tests/m2/wp6/bf22.py RUN 500` on `wp6j`: 500 non-required statuses on an open PR's head, 8 posters at a time. Then
`gh pr checks`, `GET /commits/{sha}/statuses` paged by its `link` header, the leader's core rows before and after, and
every member's core and metadata `fold_digest` at one index.

1. `bf22a` (edge `516c5fd6`): page 2 answered 404. `graftyard_wire::link` writes `/repositories/{id}/...`, and the
   edge's `rest_repo` served only `/repos/o/r/...`. Fixed in `57d1411` (`graftyard_wire::repo_path`, test
   `a_link_headers_next_page_names_a_routable_repository_path`, which fails with the `/repositories/` arm removed).
   The edge was deployed as `3a2e4f02`.
2. `bf22c`: 495 of 500 read back, though the log holds 500 committed `Status` events. The edge mints `StatusId` from
   `Date::now`, and the `status` table is keyed (sha, -id), so two statuses posted in the same millisecond on one
   commit share a key, and the second replaced the first. Fixed in `31eeadf`: `decide` takes the smallest free id at
   or above the edge's (still the status's version, so an older status still loses), returns it as `IssueOut::Id`,
   and the 201 renders that id. Test `two_statuses_minted_in_one_millisecond_keep_both_rows_and_the_later_wins`
   fails with the free-id loop disabled. Site day `sd8`.
3. `bf22e` (sd8 build): **pass**. 500 of 500 statuses through `gh pr checks` and through REST (5 pages), none
   required. Core rows unchanged other than `consumer_cursor`: `PrSync` moves its cursor every `CURSOR_STRIDE`
   entries whatever they hold, so the earlier `bf22d` run, which crossed a stride, changed only that row. All three
   members agree on both tiers' `fold_digest` at index 1,033+. The prune clause (entry time past
   `check_retention_days`) stays with the pure test, because the install's clock cannot be moved.

## BF-25 and BF-26 smoke

`RUN=wp6b1 tests/m2/edge-drive.sh .../blob1 wp6/wp3m 2 lfs release` on edge `3a2e4f02`: `git lfs push` of a
300,000-byte object, a fresh clone, and `git lfs pull` (bytes match). Then `gh release create`, `upload`, `view`,
`download`, and `download --archive=tar.gz`, all exit 0. Through the R2 API, both blobs (`blob/196631/98ea…`, 3 bytes;
`d9bc…`, 300,000 bytes) are in all three site buckets. BF-26: I deleted all 6 `archive-cache/` objects in
`graft-m1-packs-a` (the only bucket that held any) and ran `gh release download --archive=tar.gz` again. Exit 0, the
bytes match the first download (sha256 `18d11688…`), and the object `archive-cache/196631/66e3c899…-986ec71b….tar.gz`
was rebuilt. Not rerun: BF-25's "one site bucket made unreadable". The install has no hook to make a bucket
unreadable, so it stays with M2-WP4's run.

## Earlier M2 rows on main's build plus these fixes (reduced counts)

Final build: sites `7b489c03` (a), `66a640de` (b), `6a0a138e` (c) from site day `sd10` with `--roll-shards`; edge
`3a2e4f02`. Rows marked sd8 ran on the build before the two shard fixes (`576badf`, `4c3727e`), which touch only the
shard's merge path.

| Row | What ran | Count | Result |
| --- | --- | --- | --- |
| BF-1 | `hosted.py queue bf1s 2 --fail 0 --bf1` on `wp3f` | 2 runs | **Pass 2/2.** Entries 326 and 357 each hold 5 `Merged`, `Landed`, and one `Refs` (15 ref updates) under both request ids, committed on all 3 members with one digest each (`1f65a0c0…`, `ee9f6ad7…`). The first try, `bf1r`, used the default `--fail 3`, so only 4 landed; that is a driver option, not a fault |
| BF-20 | Smoke only: `/fold {"at": 1720}`, then 3 pushes and `/fold {"at": 1738}` on `repo:196628` (the 3.99 GB tier) | 2 digest checks, 3 pushes | **Smoke passes.** After two site days the three members agree on the core and metadata digests at 1720 and at 1738, and the last committed snapshot is still `to` 1313, 1,330,000 rows. The first pushes after two days idle took 7.1 and 14.4 s (cold tier load), one returned an empty reply, and the rest committed. Not run: the 300/s snapshot campaign (`load.sh` through `/bench_seed` returned `status: null` once, and the brief allows a smoke) |
| BF-21 | `hosted.py queue bf21t 10 --kill-every 2` on `wp3q2` (sd10) | 10 runs, 5 kills | **Pass 10/10.** The 3rd PR is ejected `CheckFailed`, the other four merge, and `main` moves only by `QueueMerge`. Kills `after_quorum` at 1017, 1177, 1308 and `before_commit` at 1103, 1239; each ends whole in 93 to 120 s |
| BF-21, first try | `bf21r` 10 on `wp3q` (sd7), `bf21s` on `wp3q2` (sd8) | 10 + 2 | `bf21r` 9/10. Trial 9's kill held PR 197's rebuild for 9.5 min (600 s window). `bf21s` stalled the same way. Two bugs, fixed below; the audit was all-or-nothing in every trial |
| BF-22 | `bf22.py bf22e 500` on `wp6j` (sd8) | 500 statuses | **Pass**, after the two fixes in "BF-22 reduced" |
| BF-23 | `hosted.py bf23 bf23r 10` (sd7), `bf23s 10` (sd10) on `wp3c` | 10 + 10 | **Pass 20/20** (merge 8, squash 6, rebase 6): refused "Required status check \"ci\" is in progress." until the newer success, then merged |
| BF-24 | `bf24.py wp6bf24 10` (sd7), `wp6bf24b 10 --conflict-every 3 --transfer-at 4` (sd10) | 10 + 10 | **Pass 20/20** within 30 s: clean 0.83 to 2.10 s (one 7.17 s first push in sd7), conflicts 1.03 to 1.71 s, one transfer in each |
| BF-25 | `edge-drive.sh lfs release` smoke, and the R2 objects | 1 | LFS and asset bytes match. Both blobs are in all 3 buckets. The unreadable-bucket clause was not run (no hook) |
| BF-26 | All 6 `archive-cache/` objects deleted, then `gh release download --archive=tar.gz` | 1 | **Pass.** Same bytes (sha256 `18d11688…`), and the object was rebuilt |
| BM-2 stale `parent_tip` | `hosted.py bm2 bm2f 100` on `wp3b` (sd10) | 100 | **Pass 100/100.** One client TLS handshake timeout stopped the driver at 61; it resumed |

Reply validation for these runs, 0 failed: `bf21t` 79 GraphQL and 10 REST, `bf23s` 38/3, `bf1s` 102/16.

## The kill-trial stall: two shard bugs from the merge-latency fix

`9799a1d` (merge first, catch up only on 409) moved the shard's catch-up behind the merge. The catch-up is what started
the container (`ShardHost::up`). When the container had stopped (after its inactivity timeout, or on a new image), the
`MergeQueue` consumer's `CacheAsk::Build` failed `422 The container has not been started` with no catch-up after
it. The in-band attempt failed, and so did every re-send and 30 s retry, until some other route (a push's
`/validate`, a clone) started the container. `wp3q`'s trial 9 rebuilt PR 197 only after 9.5 minutes. `wp3q2` stalled
at cursor 944 for 20 minutes, which the wrangler tail showed (`raw/m2-wp6/tail/`).

1. `576badf`: `merge_then_catch_up` takes an `up` step first. Test `a_merge_catches_up_only_when_a_commit_is_missing`
   now models a stopped container, and fails with `up` skipped ("not started").
2. `4c3727e`: after (1), the started container held no repository for the network and answered 404, which
   `merge_chain` passed up instead of catching up (the catch-up rehydrates it). `needs_catch_up` takes 404 as well as
   409. Test `a_merge_on_a_container_with_no_repository_catches_up_first` fails with 404 removed.

After `sd10`, `wp3q2`'s stuck queue rebuilt by itself (`GroupBuilt`, cursor 944 → 950) and landed once CI passed
both tips. BF-21 then passed 10 of 10 with 5 kills.

## Fixes on this branch, each with a test that fails on the old code

| Commit | Fix | Test |
| --- | --- | --- |
| `f27d27e`..`9799a1d` | Lookup reads registry rows only after the names miss; dependency reads at once; catch-up reads at once; merge before catch-up; first commit send to the cached leader | `a_lookup_reads_registry_rows_only_after_the_names_missed`, `dependency_reads_run_at_once`, `a_catch_up_reads_first_index_and_refs_at_once`, `a_merge_catches_up_only_when_a_commit_is_missing`, `a_commit_starts_at_the_cached_leader` |
| (journey routes) | GraphQL `addPullRequestReview`; REST `GET /issues`, `POST /labels` | `journey_3_label_then_issue_then_both_lists`, `add_pull_request_review_commits_a_review_at_the_head` |
| `57d1411` | REST under `/repositories/{id}` | `a_link_headers_next_page_names_a_routable_repository_path` |
| `31eeadf` | A status minted in the same millisecond keeps its own row | `two_statuses_minted_in_one_millisecond_keep_both_rows_and_the_later_wins` |
| `576badf` | Shard merge starts the container first | `a_merge_catches_up_only_when_a_commit_is_missing` (stopped container) |
| `4c3727e` | Shard merge catches up on 404 | `a_merge_on_a_container_with_no_repository_catches_up_first` |

Edge wiring changes (tiers and `Authz` at once, one resolve, no `ensure_group` before a commit, auth alongside
resolve, the owner's record to the shard, the merge plan on the leader site's shard) have no unit test. They are
measured above.

Local checks on the final commit: `cargo fmt --check` OK, `cargo clippy --workspace --all-targets -- -D warnings` exit 0,
`cargo test --workspace` 381 passed and 0 failed, `go test ./...` in `containers/git/server` ok. `crates/raft` is
untouched.

## Deployments (test-faults builds, `graft-m1-*` only)

| Site day | a | b | c | Why |
| --- | --- | --- | --- | --- |
| sd7 | `f1bd2761` | `1b448904` | `b6539846` | Merge-latency fixes |
| sd8 | `ccdfc76e` | `e7c0e891` | `451228ca` | Status id |
| sd9 | `3a7fc2e0` | `9b4c3dc3` | `a5595563` | Container start (`576badf`) |
| sd10 (current) | `7b489c03` | `66a640de` | `6a0a138e` | 404 catch-up (`4c3727e`) |

Edge `graft-m1-edge`: `516c5fd6` (leader-site shard, all latency fixes), then `3a2e4f02` (`/repositories/{id}`,
current), with `FORMATS_ENABLED=32.1,32.4,32.5`. Every site day ran with `--roll-shards`; its roll printed no
shard lines, because the container image was the same.

## Design sentences contradicted, and gaps

- **Merge latency.** BM-2's "merge p99 on a freshly elected leader is at most 2 s" is not met on this placement:
  3.20 s at p99, with a LAX client and edge, identity leaders on enam, and the repository's new leader on weur or apac.
  What remains is cross-region round trips: auth and resolve, tiers and `Authz`, the shard's merge, and the leader's
  three dependency reads plus one replication round. Only enam leaders reach 2.09 s.
- "The write path" says the leader reads `deps` "once through `fresh`, in parallel". The old code read them one
  after another (fixed, `dependency_reads_run_at_once`).
- "Commit and read paths" has the edge use a cached `GroupConfig` for the leader. The old `commit_hedged` looked the
  leader up before every first send (fixed).
- `GitCacheShard` says the network's home shard computes a merge, and journey 2 says the base site's shard. The merge
  plan now runs on the leader site's shard, as `PrSync` and the `MergeQueue` consumer already did, because that shard
  is the one kept caught up.
- SHARED, `RequiredCheck`: "a status's version is its `StatusId`", minted from the edge's clock. Two statuses minted
  in one millisecond replaced each other's row. `decide` now takes the smallest free id at or above the edge's, which
  keeps the version order but is still the edge clock's order (F51 stays open).
- Not built: journey 2's fork (Milestone 2's repositories are roots), `PATCH /repos/o/r/labels/{name}`
  (`gh label create --force` exits 1), and the merge mutation's payload, whose `pullRequest` is `null` in
  `render_merge`.
- BF-22's prune clause and BF-25's unreadable bucket have no hosted hook (the install's clock cannot be moved, and a
  bucket cannot be made unreadable), so they stay with the pure tests and M2-WP4's run.

## `bm2-latency`: the 2 s budget, and cutting the fresh leader's round trips (in progress)

Branch `bm2-latency` from `main` `0e79db8`. Progress notes, written as the work runs.

### What the 2 s was budgeted against

Nothing in DESIGN or DECISIONS derives the 2 s, places the client, or says whether the client-to-edge leg counts.

- The sentence appears only in BUILD-PLAN's BM-2 row and DESIGN's Milestones table, row 2. It came over from the
  arena candidate's design, `history/arena/cloudflare/candidate-3/DESIGN.md`, which is not normative.
- The only other "merge p99 at most 2 s" is B0-29's pass value ("Commit p99 at most 600 ms, merge p99 at most 2 s"). It
  was measured "at the leader" and "at the client" by load clients spread over "enam, enam, wnam, weur, apac", with the
  leader on enam: "Merge p99 367 to 795 ms at the leader, at most 874 ms at the client" (`tests/m0/results/m0-wp9.md`).
  Those merges had no git cache plan and no edge identity reads.
- DESIGN, "Commit and read paths": "Every request enters at the Cloudflare data center nearest the client". So the
  design's request path starts at the client's colo, and the client-to-edge leg is part of what a client sees.
- DESIGN, Latency: "Fresh reads on a held lease cost one DO call in the leader's region, plus the client's distance to
  it", and "Preferred-site transfer keeps leaders near owners." DESIGN, "Entries per group": "one read of the entry's
  `Authz` dependency on a leader in the owner's home region [guess]".

The design's model is a client near the owner's home, with the leader there too. BM-2's "freshly elected" leader,
though, is on a different site from the last one, so at least two of every three samples lead away from the owner's
home (enam). I measure as before, at the client, which is the stricter reading. The LAX client-to-edge leg is about 87 ms
of p50 (client time minus the edge's own time), so measuring from the edge would not change the verdict.

### Read-index from a same-site learner: not `fresh`, and no saving

DESIGN, "The read path": "`fresh` ... A fold that includes every entry acknowledged before `arrived`. Only the leader
builds one", and "A follower answers reads by batching read-index requests to the leader every 10 ms and waiting until
applied. A learner does the same with one request in flight and at most one every 50 ms." A learner on the
repository leader's site would still send one read-index round trip to the dependency group's leader on enam, the
same cross-region trip the leader pays now, plus up to 10 or 50 ms of batching. It saves nothing. It would also break
"`decide` sees the `deps` folds the leader read once through `fresh`", because only a leader builds `Fresh`. Not built.

### Edge passes its auth result to the leader: no saving at the leader

The leader must read the `Tokens` shard through `fresh` in `deps` itself (DESIGN Invariants: `decide`'s result depends
only on "the `deps` folds the leader read once through `fresh`"), and it reads `Tokens`, `Names` (the account chain), and
`Authz` at once, so the slowest of the three bounds the step. Skipping `Tokens` would not shorten it, and taking the
edge's word would weaken `decide`. Not built.

### Baseline on main's build (`base`, 30 samples)

Sites `3b620ff6`/`abd0eace`/`ac07bd32`, edge `b8729c3c`. n = 30, all passed, p50 2,862 ms, p90 3,513 ms, max 4,101 ms.

| New leader | n | p50 ms | auth + resolve | tiers + `Authz` + logins | merge plan (shard, its catch-up) | commit |
| --- | --- | --- | --- | --- | --- | --- |
| 0, enam | 10 | 2,157 | 489 | 398 | 788 (542, 307) | 408 |
| 1, weur | 9 | 2,862 | 437 | 473 | 1,366 (1,028, 539) | 692 |
| 2, apac | 10 | 3,457 | 564 | 807 | 993 (559, 307) | 983 |

Every one of the 29 shard merges caught up first (`catchup;desc=Some(...)`). The leader's `deps_log` shows the three
dependency reads themselves at 10 to 40 ms (enam leader), 110 to 260 ms (weur), and 215 to 430 ms (apac). Before them,
each `Authz` read waited on a `fresh` read of the owner's `Placement` row on `Names`, one more round trip to enam.

### Cuts, commit `1c84ac6`

1. The leader's `Authz` dependency is read under the record its member joined with (`owner_chain_record`). It no longer
   reads the owner's `Placement` row on `Names` first. If the joined record finds no group (the owner moved), the
   current record is read and tried. Test `the_owners_authz_is_read_under_the_joined_record`.
2. A new leadership catches up its own site's shard of the network (`prsync::warm`, on the dispatcher's first alarm of
   the leadership), so the first merge plan finds both commits already there.
3. The edge reads a merge's logins while the git cache plans it, and `repo_name` reads the owner's login and `Authz` at
   once.

### After cuts 1 to 3 (`c1`, 30 samples)

Sites `044d6889`/`9d7d77a2`/`b1a4fc1d`, edge `aeaf9288`. n = 30, all passed, p50 2,165 ms, max 2,876 ms. No shard caught
up on the merge path (0 of 30, against 29 of 29 before).

| New leader | n | p50 ms | max | auth + resolve | tiers + `Authz` | merge plan (shard) | commit |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 0, enam | 10 | 1,475 | 1,754 | 216 | 414 | 309 (233) | 344 |
| 1, weur | 10 | 2,294 | 2,642 | 359 | 627 | 680 (507) | 534 |
| 2, apac | 10 | 2,432 | 2,876 | 359 | 820 | 433 (234) | 713 |

The step that still grew with distance was "tiers + `Authz`": 400 to 1,180 ms on apac. The edge read both whole tiers
for a merge: 571 KB of JSON for `wp6p`'s 1,200 rows (`ref` 715, `pull_head` 238, `mergeability` 238, ...). From the orb,
the same read of the weur leader took 750 to 1,165 ms, against 235 to 400 ms for a 200-byte read of the same member.

### Cut 4, commit `c0a393b`: a merge reads only its rows

`mergePullRequest` reads `{"rows": {"merge": id}}`: the pull request's `issue_by_id`, `issue`, `pull`, and `pull_head`
rows, its base's `ref`, and `settings`, at most six rows. `graphql::plan` and `merge_request` read only these, and the
leader renders the reply from its own tiers. Test `a_merge_plans_from_its_rows_as_from_both_tiers` compares the plan,
the `MergeRequest`, and the refusals with both whole tiers. It fails with the base `ref` row left out.

### After cut 4 (`c2`, 30 samples)

Sites `91490a96`/`e8931593`/`b06ae6e3`, edge `289e6d4e`. n = 30, all passed, p50 1,804 ms, p90 2,131 ms, max 2,331 ms.

| New leader | n | p50 ms | max | auth + resolve | tiers + `Authz` | merge plan (shard) | commit |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 0, enam | 10 | 1,255 | 1,592 | 233 | 242 | 381 (319) | 322 |
| 1, weur | 10 | 1,867 | 2,331 | 249 | 226 | 746 (488) | 506 |
| 2, apac | 10 | 2,037 | 2,206 | 367 | 343 | 460 (263) | 756 |

Six of 30 were over 2 s (three weur, three apac). The 300-sample run follows on this build.

### Cut 5, commit `20155a1`: a merge by node id reads its rows alongside the owner's name

`mergePullRequest` names its repository by node id. The edge found it with `repo_by_id`, then read the owner's login and
`Authz` (now at once, cut 3), then the tier read. The merge's tier read needs only the repository, so it now starts with
the owner's login and `Authz` reads, and "tiers + `Authz`" drops to 0 in `server-timing` (it is inside
"auth + resolve"). Edge `1358b1a7`; sites unchanged (`91490a96`/`e8931593`/`b06ae6e3`).

### Rechecks after the read- and commit-path changes (edge `1358b1a7`, sites `91490a96`/`e8931593`/`b06ae6e3`)

| Row | Run | Count | Result |
| --- | --- | --- | --- |
| BF-23 | `hosted.py bf23 re23 10` on `wp3c` | 10 | **10 of 10 pass**: `gh pr merge` refused (exit 1) after the re-request, merged (exit 0) after the newer success |
| BF-21 | `hosted.py queue re21 10 --kill-every 2` on `wp3q2` | 10, 5 with a leader kill mid-landing (`after_quorum`) | **10 of 10 pass**: the base moved only by whole `QueueMerge` landings, the failing PR ejected every time |
| BF-1 | `hosted.py queue re1 2 --kill-every 0 --fail 0 --bf1` on `wp3f` | 2 | **2 of 2 pass**: one entry with a 5-PR `Landed` group and a `PrSync` batch; all three members hold it with the same digest |
| gh replies | `c2b-gh` (two real `gh pr merge` runs in the p99 driver) | 15 GraphQL, 2 REST | **0 failed** (`gh-replies.py`, then `validate.py` against GHES 3.22) |

Workspace checks on `20155a1`: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and
`cargo test --workspace` (424 passed, 0 failed). `crates/raft` is untouched, so the long sweep and Porcupine were not
rerun.

### The 300-sample run on the final build (`final`)

`p99.py final 300 --gh-every 25` on `wp6p`, edge `1358b1a7`, sites `91490a96`/`e8931593`/`b06ae6e3`, 14:20 to 16:02 UTC.
288 mutations and 12 whole `gh pr merge` runs. Each sample is on a new term: 288 distinct terms, `from != to` in all 288.
Every sample merged (288 of 288, gh 12 of 12). The driver stopped three times on client-side timeouts (a `git push`
over 120 s, and two TLS handshakes to the tools edge's `/status`), never on a merge reply, and resumed from its last line.

**BM-2's latency clause still fails: p99 4.10 s at n = 288** (p50 2.11 s, p90 2.93 s, 178 of 288 over 2 s).

| New leader | n | p50 | p99 | max | Over 2 s | auth + resolve + rows | merge plan (shard) | commit | shard catch-ups |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 0, enam | 85 | 1.62 s | 2.40 s | 2.40 s | 6 | 523 | 630 (565) | 350 | 3 |
| 1, weur | 105 | 2.48 s | 4.15 s | 4.19 s | 94 | 535 | 1,183 (1,004) | 600 | 38 |
| 2, apac | 98 | 2.14 s | 2.89 s | 2.89 s | 78 | 592 | 694 (492) | 715 | 4 |

Step medians are in ms, from `server-timing`. The client-to-edge leg is 88 to 91 ms p50.

**Why n = 288 settles it.** The gate needs at most 1% of samples over 2 s; 178 of 288 (62%) are over. Under "p99 ≤ 2 s"
that has probability far below 10^-100. A pass at this n would need at most 2 samples over 2 s.

**What the number is limited by.**

1. Cross-region round trips, as before. On the leader, the three dependency reads (`deps_log`) take 21 to 47 ms p50 on
   an enam leader, 118 to 132 ms on weur, and 226 to 265 ms on apac. Every identity leader (`Names`, `Tokens`, `Authz`)
   is on enam. Commit is one replication round plus those reads: 350 ms enam, 600 ms weur, 715 ms apac.
2. The shard's merge gets slower over the run. Its p50 rose from 492 ms (samples 0 to 49) to 672 ms (samples 200 to
   249), while each shard's pack count grew from about 1,200 to 1,953. The shards never repack (`gc.auto 0`, and
   nothing else runs `repack`). In the orb, `git merge-tree` on a 751-pack repository took 8.7 ms, against 2.0 ms after
   `git repack --geometric=2` (and 5.9 ms with a multi-pack index). Each merge runs about six git commands, so this is
   part of the growth. The rest is the container hop and the weur container itself: weur's shard merge is about
   twice enam's and apac's at every pack count [inference].
3. Catch-ups on the merge path came back on weur: 38 of 105 weur samples, against 3 and 4 elsewhere. The new
   leadership's `warm` catch-up (cut 2) starts on its first alarm, and the client's merge arrives about as fast as
   the election is seen, so on the slowest shard the two race and the merge catches up itself. A catch-up costs
   250 to 700 ms.

Fresh-shard runs (`c2`, `c2b`, `c3`, about 1,200 packs) show the same placement limit without the pack growth: n = 109,
enam p99 1.68 s (0 of 36 over 2 s), weur p99 3.18 s (18 of 37 over), apac p99 2.82 s (21 of 36 over).

`final-gh` replies: 92 GraphQL and 12 REST, **0 failed** (`gh-replies.py`, then `validate.py` against GHES 3.22).

### Proposed design change

2 s is out of reach for a leader elected away from the identity leaders' region with this placement. On enam, where
`Names`, `Tokens`, and `Authz` lead, the fresh-leader merge passed on fresh shards (p99 1.68 s, n = 36) and missed
narrowly on shards with about 1,950 packs (p99 2.40 s, 6 of 85 over). On weur and apac, every step pays two to four more
cross-region rounds, so p50 alone is 2.1 to 2.5 s.

Options, in the order I would take them. Each needs an operator decision, because the gate and placement are design:

1. **Say where the gate measures.** "Fresh reads on a held lease cost one DO call in the leader's region, plus the
   client's distance to it" and "Preferred-site transfer keeps leaders near owners" assume the leader is near its
   owner and its dependencies. BM-2 should say "a freshly elected leader on the group's home site, with the client in
   that region". That is what "Preferred-site transfer" restores within one lease after any election. Off-home leaders
   would get a separate, looser bound, for example 3.5 s p99, after weur's 3.18 s fresh-shard figure. Evidence: enam-led
   p99 1.68 s fresh and 2.40 s aged here, with the LAX client about 70 ms from enam.
2. **Repack the git cache shards** (a code change under the existing design: "one repack per shard at a time",
   `design/DESIGN.md`, Failure modes, git caches). A geometric repack after each catch-up that leaves more than about 50
   packs would keep the shard merge near its fresh 230 to 350 ms (enam, apac). The enam-led aged p99 should then fall
   back toward 1.7 s [inference: from the fresh-shard figures].
3. **Placement (proposal only).** Putting `Names`, `Tokens`, and `Authz` leaders near each owner's home is already the
   model: "one read of the entry's `Authz` dependency on a leader in the owner's home region [guess]". It does not help a
   repository leader that is temporarily off home. The leader's three dependency reads cost 120 to 265 ms from weur and
   apac, so co-locating them with a moved leader would cut 100 to 250 ms of commit and about as much at the edge. That
   brings weur and apac p50 to about 1.7 to 2.0 s, still not a 2 s p99. I do not recommend chasing it with placement.

### Not built, and why

- **A same-site learner's read-index for `deps`.** It is not `fresh` ("Only the leader builds one", "The read path"),
  and it still pays one round trip to the enam leader plus 10 to 50 ms of batching.
- **The edge passing its auth result to the leader.** `decide`'s result must depend only on "the `deps` folds the
  leader read once through `fresh`", and the leader's three dependency reads run at once, so skipping `Tokens` saves
  nothing.

## `bm2-repack`: shard repack and the catch-up wait (in progress)

Branch `bm2-repack` from `main` `5868bab`. The operator approved proposal 2 and the catch-up follow-up; proposal 1 is
held until this rerun, and there is no identity-placement change.

### What changed

- **`ca15db8`, a geometric repack after catch-ups.** `PUT /refs` in `containers/git/server/main.go` is the last step
  of every catch-up and rehydrate. When the network has more than `repackAt = 50` packs [guess], it starts
  `git repack -d -q --geometric=2` in the background. A process-wide `TryLock` keeps it to one repack per shard
  container at a time ("one repack per shard at a time", `design/DESIGN.md`, Failure modes, git caches). The repack
  holds the network's write lock, so a merge or upload never reads a half-replaced pack set. The reply gains
  `"repack": true|false`. `TestRefsRepacksPastRepackAt` fails with the trigger removed. In the orb, 2,000 packs
  repacked in 0.75 s the first time and in about 12 ms after that.
- **`e8d47ad`, the first merge waits for the in-flight catch-up.** The leader site's shard counts its running
  catch-ups per network (`CatchUps` in `crates/git/src/merge_cache.rs`). Every `host.catch_up` in
  `workers/site/src/shard.rs`, including the new leadership's `warm` on `/catchup`, holds a `Running` guard.
  `merge_then_catch_up` takes an `in_flight` future, and `/merge` awaits `idle()` on it before the first merge plan.
  `x-graft-timing` gains `waited;dur=`.
  `a_merge_waits_for_the_catch_up_in_flight_instead_of_racing_it` fails when the await is removed. The
  "home shard" wording in `workers/edge/src/api.rs` comments now says the leader site's shard of the network.
- **`4b6631f`, no token on argv or in logs.**
  - `hosted.py` and `bf24.py` passed `http.extraHeader=Authorization: Basic …` on git's command line. A
    `TimeoutExpired` traceback printed it into `raw/wp5/base1.log` and `dist.log`.
  - git now takes the header from `GIT_CONFIG_COUNT` env, and curl in the shell drivers from `-H @<(printf …)`.
  - Every log line, jsonl row, gh capture, and uncaught traceback in the two Python drivers passes through `redact()`.
  - The old wp5 logs, and three `smoke*.work/.git/config` files from an older bf24 that held the token in the remote
    URL, are redacted in place (gitignored).

Checks: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (439
passed), `go vet` and `go test` in `containers/git/server`. Image
`graft-m1-site-gitshard-git@sha256:8fa9abd46a0a848bc5d819fdc8eb09b3eaabda4aaf7ec90ae84c57f7e90c0891`.

### Deploy (site day `bm2-repack`)

- Sites: `graft-m1-site-a`, `b`, and `c` went through two phases each. The final versions are a `bec631b6`, b
  `06b4db06`, and c `c90060c1`. They carry the image above and are the `test-faults` build of `9301e15`
  (`raw/bm2-repack/siteday.log`).
- Edge: `graft-m1-edge` `d0b2bb32`, with `FORMATS_ENABLED=32.1,32.4,32.5` (`raw/bm2-repack/edge.log`).
- `--roll-shards` rolled 10 instances, then stopped on its own 300 s curl timeout. The timeout was on `main`'s
  (196608) catch-up, which ended on its own: that instance runs the new image. `raw/bm2-repack/roll2.log` rolled the
  last 5 running instances with the same `/_bed/kill` and `/_bed/catchup`.
- The catch-ups of `k0` and `k1` (196611 and 196612) answer 500 on both shards: "git update-ref: … nonexistent object"
  for `refs/heads/bf12/p1`. The same 500 shows on the m2-wp6 roll (`raw/m2-wp6/sd/deploy-site.log`), before this
  branch, so it is a pre-existing M1-era state of those two repositories. No gate uses them.
- Repack, live: every `wp6p` shard went from 1,955 packs to 1 after its first catch-up on the new image
  (`/_bed/fsck`: `in-pack: 3916, packs: 1`).
- Each cold rehydrate of 1,955 inline packs took about 100 to 120 s. During that time `git fetch` on `wp6p` timed out
  at 120 s, and the sanity run's first try stopped on it. A container replaced on the new image fills itself again
  from R2 and the log, so this is the one-off cost of the roll.

### Sanity run (`sane`, 30 samples)

29 GraphQL mutations and 1 `gh pr merge`, all merged. 30 distinct terms, `from != to` in all 30. p99 1.86 s (p50
1.56 s, max 1.86 s), per leader: enam n = 5, p99 1.48 s; weur n = 11, 1.84 s; apac n = 13, 1.86 s. The shard merge
is p50 105 ms (max 530 ms), against 492 to 1,004 ms before the repack. 3 samples waited for an in-flight catch-up,
and none caught up on the merge path (38 of 105 weur samples did before).

### The 300-sample run on the `bm2-repack` build (`final`)

`p99.py final 300 --gh-every 25` on `wp6p`, edge `d0b2bb32`, sites `bec631b6`/`06b4db06`/`c90060c1`, 17:46 to 19:30
UTC. That is 288 GraphQL `mergePullRequest` mutations (gh's own `PullRequestMerge` document) and 12 whole
`gh pr merge --merge` runs. Every sample is on a new term: 300 distinct terms, `from != to` in all 300. Every sample
merged: 288 of 288, and gh 12 of 12. The driver stopped three times on client-side TLS handshake timeouts and resumed
from its last line. One of the three was the merge POST of the sample after 4, which the driver never recorded and
made again as a new sample. The other two were the tools edge's `/status` and a GraphQL read.

**As the gate is written (all leaders, client at LAX): p99 2.42 s at n = 288. BM-2's latency clause still fails.**
p50 is 1.52 s, p90 1.78 s, and max 2.86 s. 8 of 288 samples are over 2 s; a pass at this n allows at most 2. If the
true p99 were 2 s, 8 or more over would have probability 0.009.

| New leader | n | p50 | p90 | p99 | max | Over 2 s | auth + resolve | merge plan (shard) | commit | waited | shard catch-ups |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| all | 288 | 1.52 s | 1.78 s | **2.42 s** | 2.86 s | 8 | 540 | 268 (116) | 540 | 21 | 0 |
| 0, enam | 66 | 1.31 s | 1.49 s | **2.01 s** | 2.01 s | 1 | 532 | 235 (159) | 384 | 15 | 0 |
| 1, weur | 120 | 1.50 s | 1.69 s | **1.96 s** | 2.08 s | 1 | 540 | 296 (123) | 529 | 6 | 0 |
| 2, apac | 102 | 1.62 s | 1.85 s | **2.71 s** | 2.86 s | 6 | 548 | 250 (56) | 716 | 0 | 0 |

- Step medians are in ms, from `server-timing`. "Merge plan" is the edge's step, and the parenthesised figure is the
  shard's own `merge;dur`.
- "waited" counts samples where the merge waited for the new leadership's catch-up. The waits were 4 to 198 ms.
- "Shard catch-ups" counts samples that caught up on the merge path.
- n per site is uneven because the gh samples fall on a fixed step of the leader rotation.

**Home site only (enam) with the client distance:** p99 2.01 s at n = 66, with 1 of 66 over (2,014 ms, sample 212).
The client is in LAX (`cdn-cgi/trace` `colo=LAX`), and the edge runs in LAX. The client-to-edge leg is client time
minus the edge's summed `server-timing` steps: 100 ms p50 on enam samples (98 to 100 ms on every site). From the edge
(minus that leg), the enam p99 is 1.93 s, with 0 of 66 over. The LAX-to-enam distance shows in the leader's steps,
not the client leg: the edge in LAX makes every identity and leader call to enam. n = 66 is too small to call a
p99 either way. The 2.01 s point is one sample, and a p99 at n = 66 is the maximum.

**Before and after, the same gate reading:** p99 4.10 s at n = 288 before; 2.42 s at n = 288 now. Over 2 s went from
178 of 288 to 8. weur went from p99 4.15 s to 1.96 s, apac from 2.89 s to 2.71 s, and enam from 2.40 s to 2.01 s. The
shard's own merge went from 565, 1,004, and 492 ms p50 (enam, weur, apac) to 159, 123, and 56 ms. Catch-ups on the
merge path went from 45 of 288 (38 on weur) to 0 of 288.

**What limits it now.**

1. apac commit, 716 ms p50: one replication round from apac plus the leader's three dependency reads to enam, about
   230 to 265 ms each way.
2. Edge-to-shard tail on apac. In 2 of the 6 apac samples over 2 s, the edge's merge-plan step was 839 to 889 ms
   while the shard's own merge took 53 to 64 ms. The time went to the LAX edge's call to the apac site's shard object
   and container, not to git.
3. auth + resolve, about 540 ms p50 everywhere: the edge's `Tokens`, `Names`, and `Authz` reads to their enam leaders
   from LAX. It is the same on every leader site, and it is the largest step for enam and weur.

Removing the client leg would still not pass the gate as written: from the edge, p99 is 2.32 s with 3 of 288 over.

**`gh pr merge` whole runs** (12): p50 3.71 s, max 4.49 s, 12 of 12 merged. A whole run is gh's start-up plus its
reads before the mutation, so it is not the merge latency. The gate figure is the mutation above, as in every earlier
run. `final-gh` replies: 93 GraphQL and 12 REST, **0 failed**; `sane-gh` 8 and 1, 0 failed (`gh-replies.py`, then
`validate.py` against GHES 3.22).
