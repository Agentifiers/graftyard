# M2-WP6: the Milestone 2 gate (BM-2) on `graft-m1-*`

Work in progress. These notes are written as the runs go, so a lost session keeps what was measured.

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

