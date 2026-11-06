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

Stable leader, `wp3m`, after the lookup and dependency fixes: `mergePullRequest` 4.9 s, then 1.6 to 1.7 s.

Fresh leader (`tests/m2/wp6/p99.py`, leadership moved with `/transfer` before each sample):

| Build | n | p50 | max | Notes |
| --- | --- | --- | --- | --- |
| lookup + deps fixes | 6 | 2.36 s | 3.89 s | `try2` |
| + catch-up, edge reads | 9 | 2.40 s | 2.73 s | `try3`; apac leader slowest |
