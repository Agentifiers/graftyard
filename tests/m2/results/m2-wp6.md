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
