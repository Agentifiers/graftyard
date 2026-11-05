# M2-WP5: cursors, Queues, and the `PrSync` consumer (BF-24)

Run on 2026-11-04 against the test install `graft-m1-site-a/b/c` (enam, weur, apac; site ids 0, 1, 2) and `graft-m1-edge`, on branch `m2-wp5`. The build was `test-faults`. The gate row is in `build/BUILD-PLAN.md`, M2-WP5:

> BF-24 | A head push, then `git ls-remote origin refs/pull/N/merge`; a conflicting push | The `TestMerged` commit within 10 s with a matching `mergeable`; on a conflict the ref is deleted and `mergeable: false` | A stale ref

| Clause | Result |
| --- | --- |
| Head push, then `git ls-remote origin refs/pull/N/merge` shows the `TestMerged` commit | 50 clean head pushes with a steady leader: 46 within 10 s, p50 5.57 s, p90 8.64 s, max of those 9.28 s. The other 4 (8%) took 39.2 to 40.3 s on the server, because their queue message sat in Queues for 36.5 to 37.6 s. Each merge commit's parents were (base tip, head) |
| With a matching `mergeable` | 130 of 131 leader reads over all runs matched: `true` for a clean head and `false` for a conflicting one. The other read, trial 69 of `dist`, came before its late `TestMerged` (40.3 s) and read `null` (GraphQL `UNKNOWN`), which is the right value for a head with no test merge yet. The audit checks every `TestMerged`'s outcome, and all 131 are right |
| A conflicting push deletes the ref, and `mergeable: false` | 25 conflicting pushes: 25 within 10 s, p50 5.91 s, max 9.35 s. The ref was gone and `mergeable` false each time |
| No stale ref | The log audit passes: each of the 131 pushed heads has exactly one `TestMerged` with the right outcome, and `refs/pull/1/head` moved to each head once. Both refs move in the same `PrSync` entry, so a reader never sees the new head with the old head's merge. Until that entry commits, both still show the previous head. That is the latency above, and it is not a ref left behind |
| Leader change mid-stream | 30 trials with 13 leader transfers, 3 of them with the old leader reset and restored, and 6 with the old leader's queued batch purged. Nothing skipped, nothing applied twice, and the cursor never went back. Latency with a transfer was 6.4 to 11.8 s. With the batch purged it was 13.9 to 16.1 s, most of it the harness's own pause, purge, resume, and transfer. Steady trials between them ran p50 6.2 s and up to 10.2 s on the server, slower on the weur and apac leaders |
| **Verdict** | **Fail on the 10 s clause, pass on correctness.** p99 is over 30 s: 4 of 50 clean pushes missed the gate, all from Queues holding a message for about 37 s, which this package's 30 s re-send then covered. On the enam leader, 92% of clean pushes and every conflict met 10 s. Correctness held in every run: the merge ref, `mergeable` once its `TestMerged` committed, the conflict deletion, and the cursor across leader changes and restores. See "Where the time goes" for what limits it and the two changes that would bound it |

## What was built

Commits on `m2-wp5`, from `529f740`:

- `bf56d11` Git cache. The container's `/merge`: `git merge-tree --write-tree`, then a deterministic `commit-tree` (parents base and head, the head's committer date, a fixed identity), then DESIGN's tree-diff check: the merge's change against each parent must be exactly what the other side changed since the merge base. It answers the conflicted paths, or the merge commit and a pack of only the objects the merge added. The merge runs in a quarantine object directory, so a refused merge leaves nothing behind. The Go test uses real git, and a planted no-op check and a pack of all reachable objects are each caught.
- `80970e4` The pure half, `crates/git/src/prsync.rs`.
  - `RepoEvent::TestMerged` (tag 21) records `Mergeability { base, head, result }` and moves `refs/pull/N/merge`, or deletes it on a conflict.
  - `RepoEvent::ConsumerCursor` (tag 22) commits a consumer's position. Both are CORE-tier rows (`mergeability`, `consumer_cursor`).
  - `Dispatcher`, the leader's memory-only half, described below.
  - `PrSync::decide_sync` and `RepoCmd { Push, Sync }`.
  - GraphQL `mergeable` returns `MERGEABLE`, `CONFLICTING`, or `UNKNOWN`. `mergeStateStatus` gains `DIRTY` and `CLEAN`. Both read the recorded test merge only while it is of the pull request's current head and its base's current tip (`prsync::mergeable`). Before this, `crates/api` always answered `UNKNOWN`. `crates/api/tests/surfaces.rs` covers it.
- `7a54bb7` Request ids. A `PrSync` command's id is the SHA-256 of its canonical content, with tag `0x5059` in the top 16 bits. A redelivered batch planned on the same fold resends the same ids, which the request rows answer. A retry planned on a fold its first attempt changed gets new ids, so it is decided again instead of answered from an unrelated row.
- `fdf74de` Site wiring, `workers/site/src/prsync.rs`.
  - A leading repository member's alarm runs 50 ms after it applies entries, runs the dispatcher, and sends each batch to its own site's queue (`PRSYNC`).
  - The site script's `#[event(queue)]` handler is the consumer Worker. It reads the batch's pull requests fresh from the leader (`/read {"prsync": [n]}`), has this site's git cache shard of the network run `/merge`, and commits the batch's `RepoCmd::Sync` commands through `commit_as` under their content ids. A message is acked only after its last command, which moves the cursor, commits. Anything else is `retry()`.
  - `deploy.mjs` creates `graft-m1-prsync-a/b/c` and binds each as its site's producer and consumer: `max_batch_size` 10, `max_batch_timeout` 0, `max_retries` 10, `retry_delay` 2.
  - The repository family reads formats 32.1 and 32.3.
  - Test installs open a pull request with `/bench_seed {"openPr": ..}`, because the edge has no route that opens one yet.
- `5f43a65` `tier::reshape` creates a fold table that a release adds. See "The first deploy's restore incident".
- `3b30017` `SyncRead`, one serde type for both sides of the consumer's read. The first deploy's consumer decoded a tuple where the route wrote an object, and failed every batch. The pipe test now reads through the JSON.
- `79dd08a` Test-faults `/log` shows each entry's `at`, the ref names it updates, and its `TestMerged`s, so the driver can time a push on the leader's clock. `grep-faults.sh` gains the needles.
- `44f9eaa` The queue message carries the chain's `PlacementRecord`, which the leader joined with. The consumer had read the owner on `Names`, then the owner's record on `Authz`, about 0.8 to 1.6 s of each batch. A fresh consumer isolate missed its 60 s cache every time. A message with no record is still read the old way.
- `81b6d77` `tests/m2/bf24`: the driver, the summary, the log audit, the consumer-log dump, and the planted-bug runner.

`PrCapture` was already built by M2-WP2. `PrOpened` captures it, and this package reads it unchanged.

### The trigger and the batch

`prsync::touched` names the pull requests an entry asks `PrSync` to look at: a `Refs` update to an open pull request's head branch, a `PrOpened`, or a reopen. A base branch's move is not a trigger. DESIGN runs a test merge "on a head push, on Actions dispatch ..., and on a single-PR GET" (Shape, `TestMerge`), so after a base move `mergeable` reads `UNKNOWN` until the next head push. `a_base_move_leaves_mergeable_unknown_until_the_next_head_push` covers this.

A batch is `SyncBatch { from, through, prs }`, the pull requests that entries `(from, through]` touched. The consumer's `commands` plan holds the head moves (`refs/pull/N/head` to the current head, for same-repository pull requests), the test merges, and, last, the cursor move to `through`. A batch with more than `MERGE_BYTES_PER_COMMAND` of merge packs splits into several commands, and only the last moves the cursor. A merge whose pack alone is over the bound is not recorded. Its merge ref is deleted, so no ref is left on an older head.

`decide_sync` is pure and keeps only what still holds on the fold:

- a batch whose `through` the cursor has passed commits nothing;
- a head move happens only to the pull request's current head;
- a test merge is recorded only for the current head, and only once per (base, head);
- the cursor only moves forward.

So a batch delivered twice, or out of order with a later one, changes nothing the first delivery did not.

## How a cursor survives a leader change and a restore

The invariant is "an event exists if and only if its change committed" (AGENTS.md). The cursor is built so that its position also exists only as committed state:

- **The position is group state.** The cursor is the CORE-tier row `consumer_cursor[PrSync]`. Only a committed `ConsumerCursor` event writes it, in the same entry as the head moves and test merges it covers. A new leader, a follower, and a member restored from its storage all read the same row, because it is part of the fold that every member applies and that `fold_digest` covers. No outbox exists. The dispatcher reads the log itself, from the member's memory or the stored log (`stored_entry`).
- **The leader's half is memory only and always restarts at the cursor.** `Dispatcher` keeps `scanned`, the pull requests each unhandled entry touched, and the last send. It is keyed by (member, term). A new term, a reset isolate, or a restored member starts a fresh one, and `to_read` starts reading at the committed cursor. Every batch starts at the committed cursor (`from = cursor`) and names every pull request touched after it. So entries that a lost batch covered are covered again by the next batch, whoever sends it.
- **A lost batch is only late.** A batch that the cursor has not passed `REDELIVER_AFTER_MS` (30 s) after its send is sent again. This covers a message lost with its consumer, a message lost with the old leader's queue, and a leader change.
- **Redelivery is idempotent.** Content-derived request ids and `decide_sync`'s checks above make a duplicate commit nothing. Queues' at-least-once retries, the leader's re-sends, and two consumers racing on overlapping batches all converge on one `TestMerged` per head.

`a_new_leader_reads_from_the_committed_cursor_and_skips_nothing` drives this on three real Raft members. A head push's batch is lost, then the leader fails. The new leader commits more work. The old leader restores from its storage, holds the same cursor, and then leads again and syncs a further push. `assert_once` checks that each head got exactly one `TestMerged`. The hosted leader-change run is below.

## BF-24

### Method

`tests/m2/bf24/bf24.py RUN TRIALS --conflict-every 3` on repository `wp6/bf24` (`repo:196629`, leader on site 0, enam), with pull request #1 from `refs/heads/pr` to `main`. Each trial does the following:

1. Make a new head commit on `pr`. Every third commit edits the line that `main` changed, so it conflicts with the base. The others keep the base's line and change another file.
2. `git push --force` it with real git through `graft-m1-edge`.
3. From the moment the push returns, start a `git ls-remote origin refs/pull/1/* refs/heads/main` every 0.5 s, overlapping.
4. Take the trial's latency as the end of the first poll that shows `refs/pull/1/head` at the new head and `refs/pull/1/merge` moved off the previous merge (clean) or gone (conflict).
5. Then check correctness. For a clean head, fetch `refs/pull/1/merge` and check that its parents are (base tip, head). Read the leader's `mergeable` through `/read {"prsync": [1]}`, which is the value GraphQL renders. A trial passes only with a matching `mergeable`: true for clean, false for conflict.
6. Read the leader's `/log` and record, on the leader's clock, the entry that moved `refs/heads/pr` (the push) and the entry with that head's `TestMerged`. This is the "server" time, which excludes the client's poll.

`tests/m2/bf24/audit.py` then reads the committed log over every trial's entries. It checks that each pushed head got exactly one `TestMerged` with the trial's outcome, that `refs/pull/1/head` moved to it exactly once, and that the cursor that each trial read never went back.

### Distribution

Run `dist` (`raw/wp5/dist.jsonl`): 75 head pushes, every third one conflicting. The leader was site 0 (enam) throughout. Latency is from the push's return to the first `ls-remote` that showed the result. A trial that never showed it within the driver's 30 s counts as over 30 s.

| Trials | n | Within 10 s | p50 | p90 | p99 | Max |
| --- | --- | --- | --- | --- | --- | --- |
| Clean head | 50 | 46 | 5.57 s | 8.64 s | over 30 s | over 30 s (server 40.3 s) |
| Conflicting head | 25 | 25 | 5.91 s | 7.67 s | 9.35 s | 9.35 s |
| All | 75 | 71 | 5.69 s | 8.20 s | over 30 s | over 30 s |

The server time, on the leader's clock from the push's entry to the `TestMerged` entry, has p50 5.05 s for clean heads and 5.15 s for conflicts. The 4 trials over 10 s (9, 15, 27, and 69) took 39.5, 39.2, 39.5, and 40.3 s on the server. The client saw a result p50 0.79 s and at most 1.71 s after the server time: the 0.5 s poll interval and part of a 4.9 s `ls-remote` through the edge.

`git push` itself took p50 8.96 s to return through `graft-m1-edge`, and the clock starts after it returns. The push's entry is committed before the push returns, so the server time, which starts at that entry, can only be longer than the time from the push's return. The client latency minus the server time (p50 0.79 s) also absorbs any offset between the orb's clock and Cloudflare's.

Earlier runs on the same build, before `44f9eaa` (`raw/wp5/base1.jsonl`, `b1.jsonl`, `base2.jsonl`, 26 trials), had p50 8.5 s. That slower p50 led to `44f9eaa`. One of their trials sat in the queue for 26.7 s, the same pattern as below. The log audit covers them too.

### Where the time goes

From the consumer's own log lines (`raw/wp5/dist.consumer.jsonl`, one per handled message) matched to the trials on the leader's clock, for the 75 trials of `dist`:

| Step | p50 | p90 | Max |
| --- | --- | --- | --- |
| Push entry applied to queue send (alarm at 50 ms, scan, `send`) | 321 ms | 382 ms | 516 ms |
| Queue send to the consumer's handler (Queues delivery) | 2,025 ms | 5,141 ms | 37,558 ms |
| Owner and record lookup | 0 ms (carried in the message) | 0 ms | 0 ms |
| Fresh read of the pull request on the leader | 417 ms | 551 ms | 1,099 ms |
| `/merge` on the enam shard, including its catch-up to the push | 1,899 ms | 2,074 ms | 2,275 ms |
| The `PrSync` commit | 606 ms | 823 ms | 2,347 ms |

The queue delay dominates. `max_batch_timeout` is 0, and the queue held at most 1 message (Queues analytics, `queueBacklogAdaptiveGroups`). Still, delivery took 0.7 to 5.5 s, and in 4 of 75 messages about 36.5 to 37.6 s. In each of those 4, the handler ran twice within a second, on the original message and on the leader's re-send from 30 s later, delivered together. The second copy found the cursor past it and committed nothing (`{"skipped": ..}`). Queues analytics shows no retries (`retryCount` 0) and no failed deliveries in those minutes. So the 37 s is time before the first delivery, not a retry. The cause is not known. A `max_batch_size` of 1 (run `b1`) did not change it: one of 8 messages there sat 26.7 s.

With delivery at its p90 and everything else at p50, a head push is about 8.3 s. So the gate holds only while Queues delivers within about 6 s. Two changes would bound it, and both are design decisions, not made here:

- A shorter `REDELIVER_AFTER_MS`. At 5 s, a held message would be covered by the re-send at about 5.3 s after the push, and the trial would finish at about 9.5 s. The redelivered batch is idempotent, so the cost is extra handler runs, not extra commits.
- Skip the queue for the head push itself: the leader's alarm could call the consumer path directly (`/merge`, then commit), with the queue kept as the at-least-once path for everything that fails. That changes DESIGN's webhook-shaped path ("Leader alarm, cursor, Queue message, consumer Worker").

Leader placement matters too. In the leader-change run, steady trials took p50 4.3 s on the server with the enam leader (8 trials), 6.2 s with weur (6), and 8.0 s with apac (3, up to 10.2 s). The `/merge` took p50 1.96 s on the enam shard (16 messages), 2.80 s on weur (13), and 4.02 s on apac (6). The first `/merge` on the weur and apac shards took 11.8 and 15.0 s, because those shards had not held the network before and caught up from the log and the buckets. Those first calls are trials 2 and 5. The commit round is also longer from a far consumer.

### Leader change mid-stream

Run `lc` (`raw/wp5/lc.jsonl`): 30 head pushes, every fourth one conflicting, with leader changes injected right after the push.

- **Transfer** (trials 2, 5, 9, 13, 17, 21, and 25). Leadership moves to the next site (member `/transfer`) right after the push. The old leader may already have sent the head's batch to its own site's queue, or it may not have. The new leader's dispatcher is new and starts at the committed cursor.
- **Transfer plus reset** (trials 9, 17, and 25). The old leader (site 0) is also reset (member `/reset`). It restores from its storage, and at the next lost-batch trial (11, 19, and 27) it is the leader that loses its batch. In trials 15 and 23, a restored member leads again.
- **Lost batch** (trials 7, 11, 15, 19, 23, and 27). Delivery on the leader's site queue is paused before the push. The queue is purged 3 s after the push returns, then resumed, and leadership is transferred. The batch the old leader sent is gone, and only a leader that starts at the committed cursor sends that head's batch again.

| Trials | n | Latency (client) | Server | Result |
| --- | --- | --- | --- | --- |
| Transfer, batch kept | 7 | 6.37 to 11.83 s | 5.6 to 11.0 s | Every one synced once |
| Lost batch, then transfer | 6 | 13.87 to 16.08 s | 13.4 to 15.5 s | Every one synced once, by the new leader's first batch from the cursor |
| No change, between them | 17 | p50 6.92 s, max 10.96 s | p50 6.21 s, max 10.15 s | Every one synced once |

`audit.py` on `lc` and on all runs together, 131 heads over entries 52 to 353: every head has exactly one `TestMerged` with the right outcome. `refs/pull/1/head` moved to each head once. The cursor that each trial read never went back. The terms went from 2 to 12 over the run.

In every lost-batch trial, the message that synced the head came from the new leader's queue (site a or b) with `from` set to the cursor before the push. For example, trial 7 pushed entry 291, the queue `graft-m1-prsync-c` was purged, and site a's consumer handled `289..292`. The new leader did not wait 30 s, because its dispatcher was new and had sent nothing, so `batch` sent as soon as it scanned. The latency of 13.9 to 16.1 s is the 3 s wait before the purge, about 3 s for each `wrangler` call, and the transfer.

So the cursor survives a leader change and a member restore on the hosted install. The 10 s gate is not met while the batch is lost, and these trials are not part of the gate's distribution.

## Tests and planted bugs

`crates/git/tests/prsync.rs` has 10 tests on three real Raft members (`graftyard_raft::Member` with the repository fold, the real `RepoCmd` codec, and a fake git cache that records how often it merged). Each test drives the dispatcher, the queue, the consumer's plan, and the commits through the members. The tests:

- `a_head_push_moves_both_pull_refs_and_a_conflict_deletes_the_merge_ref`
- `a_base_move_leaves_mergeable_unknown_until_the_next_head_push`
- `a_merge_too_large_to_record_deletes_the_older_heads_merge_ref`
- `a_new_leader_reads_from_the_committed_cursor_and_skips_nothing`
- `a_redelivered_batch_applies_once`
- `a_late_command_of_an_older_head_moves_nothing_back`
- `a_consumer_overtaken_by_a_head_push_records_nothing_of_the_older_head`
- `a_reopened_pull_request_is_synced_at_its_branch_tip`
- `the_member_wire_form_reads_a_push_and_a_sync`
- `a_batch_split_over_two_commands_moves_the_cursor_when_its_retry_is_planned_again`

`tests/m2/bf24/plant.py` plants each bug below in the source, runs the tests, and restores the file. Every plant was caught on `81b6d77` (`raw/wp5/plant.log`):

| Bug the brief asks for | Plant | Caught by |
| --- | --- | --- |
| A consumer that skips an event after a leader change | A new dispatcher starts at the leader's applied index, not the cursor | 9 tests, including `a_new_leader_reads_from_the_committed_cursor_and_skips_nothing` |
| | A batch starts at the dispatcher's last send, not the cursor | 9 tests, as above |
| | The cursor is never committed (the leader's memory is the only position) | 4 tests, including `a_new_leader_reads_...` and `a_redelivered_batch_applies_once` |
| Double-applying a redelivered batch | `decide` records a test merge that is already recorded | `a_redelivered_batch_applies_once` |
| | The "cursor has passed this batch" check is removed | `a_redelivered_batch_applies_once`, `a_late_command_of_an_older_head_moves_nothing_back` |
| | Request ids per (batch, part), so a retry on a changed fold is answered from the first attempt's row | 3 tests, including `the_member_wire_form_reads_a_push_and_a_sync` |
| A stale merge ref after a head push | A head move is not a trigger | 6 tests |
| | A merge of an older head is recorded | `a_consumer_overtaken_by_a_head_push_records_nothing_of_the_older_head` |
| | A merge too large to record leaves the old head's merge ref | `a_merge_too_large_to_record_deletes_the_older_heads_merge_ref` |
| A conflict that leaves the ref in place | `apply` of a conflicting `TestMerged` does not delete `refs/pull/N/merge` | `a_head_push_moves_both_pull_refs_and_a_conflict_deletes_the_merge_ref` |
| | `mergeable` reads a conflict as true | the same |

The container's `/merge` has its own Go test (`containers/git/server/merge_test.go`). A planted no-op tree-diff check and a pack of every reachable object were each caught.

## Checks

On `81b6d77` (`raw/wp5/checks2.log`):

- `cargo fmt --check`: ok.
- `cargo clippy --workspace --all-targets -- -D warnings`: ok.
- wasm32 clippy with `-D warnings` for `graftyard-site` and `graftyard-edge`, with and without `test-faults`: all 4 ok.
- `cargo test --workspace`: 41 suites, all `ok`.
- `go test` in `containers/git/server`: ok.
- Product-build greps. Site: 0 of 45 needles in a release build of `81b6d77`, and 45 of 45 in the deployed `test-faults` build. Edge: 0 of 20 in a release build, and 20 of 20 in the deployed `test-faults` build.
- `crates/raft` is unchanged, so the long sweep and Porcupine were not rerun.


## What is deployed

- `graft-m1-site-a/b/c` run the `test-faults` build of `44f9eaa` (deploy 8, `raw/wp5/deploy-site8.log`; site c version `db8ed447`), with the image digest `sha256:5be873a0...` from `bf56d11`.
- Queues `graft-m1-prsync-a`, `-b`, and `-c` exist. Each is bound to its site as producer and consumer with the settings above. For one short run (`b1`), `max_batch_size` was set to 1 through the Queues API, then set back to 10.
- `graft-m1-edge` runs version `b3e6ce30`, built from `fdf74de`'s crates. Its wasm is byte-identical to a fresh `test-faults` build of `81b6d77`.
- `repo:196629` (`wp6/bf24`) holds pull request #1 and every trial's commits. `graft-m1-lin-*` was not touched.

## The first deploy's restore incident

The first deploy of this package added the CORE tables `c_mergeability` and `c_consumer_cursor`. `tier::reshape` read a missing table's columns as empty, treated it as a table with rows to keep, and failed with `no such table: c_mergeability`. So every repository member on all three sites refused restore (`member restore failed: Error: no such table: c_mergeability: SQLITE_ERROR`, from 21:01Z) for about 15 minutes, until the fix in `5f43a65` was deployed. The same error shows from 20:58Z naming `c_counter`, a table that M2-WP2 added. The fix covers any table a member does not have yet. The fix creates a table that has no columns, since there is nothing to keep. Restores have been clean since.

A different restore error shows in Workers Logs on all three sites, 32 times each over the last 24 hours, from 01:32Z onward: `member restore failed: raft_state: Hit the end of buffer, expected more data`. It began long before this package's first deploy, and it hits the same member names on every site. One instance at 22:16Z came right after deploy 8. `repo:196629` was not affected: its three members answered, and one request elected a leader. This package does not touch `raft_state`. It is reported here and not investigated.

## The wasm `unreachable` in `host::commit_tx`

Not seen. Workers Logs on `graft-m1-site-a/b/c` over the whole session (from 19:40Z) have no `unreachable`, `panicked`, or `commit_tx` message. The only `Critical error`s were 3 per site at 19:46Z, before any of this package's deploys: `Uint8Array::copy_to` inside `RequestSql::rows` during a `Register` member's restore. That is a different stack.

## Design gaps

Quoted, not edited. `design/` and `decisions/` were not touched.

1. **Failure modes for the `PrSync` consumer and its per-site queue.** DESIGN's `event pipeline` rows cover the generic pipeline, for example "Queues pre-created per site", "cursors on the lost site resume on the new leader", and "A stopped consumer or sequencer only delays". Deployment modes has "event pipeline | Queues per site". No row names the `PrSync` consumer or the git cache's part in it. The consumer calls this site's `GitCacheShard` `/merge` and commits back to the chain. So a git cache that is down, or a shard that is still catching up, stops `mergeable` and `refs/pull/N/merge` for that site's leaders, while `refs/pull/N/head` waits with them because both ride the same command. Rows needed, at least for instance (a consumer isolate or shard lost mid-batch: the leader re-sends after 30 s, measured below), region, bad-deploy (a consumer that decodes nothing acks nothing; this happened in this package's first deploy), and noisy-tenant (one network's merges on a shard).
2. **The cursor type.** SHARED's "Event fan-out" defines `HookCursor` for `Webhooks` only. Nothing names how other consumers hold position. This package uses `RepoEvent::ConsumerCursor { consumer, through }` in the chain's own CORE tier. SHARED should name it, or name the type it should be.
3. **Format 32.3 is read without `FormatsEnable`.** `family.rs` reads `REPO_FORMAT_V3` at once, because only this install's own site scripts write it. SHARED's two-release rule says a reader ships a release before any writer. The rule, as it applies to commands that a site script both writes and reads, is not stated.
4. **Guessed constants.** `MERGE_BYTES_PER_COMMAND` 900,000, `REDELIVER_AFTER_MS` 30 s, `CURSOR_STRIDE` 1,000 entries, `ALARM_MS` 50 ms, `WAIT_MS` 5 s, `SCAN_MAX` 2,000, the consumer's record cache of 60 s, and the queue settings (batch 10, timeout 0, 10 retries 2 s apart). All are tagged [guess] in code. The measurement below argues for a shorter `REDELIVER_AFTER_MS`.
5. **No edge route opens a pull request.** BF-24 opened #1 with a test-only `/bench_seed`.
6. **Entries archived before the first cursor are not scanned.** A chain whose log was archived and truncated before its first `PrSync` cursor starts at `first_index`. The pull requests that those entries touched are synced at their next trigger, not at once.
7. **`refs/pull/N/head` waits on the test merge.** SHARED, "Known deviations from GitHub", has the check "After a push to a PR branch, `git ls-remote origin refs/pull/N/head` shows the new SHA within 10 s", and "Issues and pull requests" says `PrSync` moves it "in a coalesced bot `Refs` entry". This build moves it in the same `PrSync` command as the `TestMerged`, so the head ref is exactly as late as the merge ref, including the 4 trials of about 40 s. Moving the head first, in its own command, would cut the head ref's latency by the merge time (about 1.5 to 2.5 s). Nothing in the design says which to do.

## Reproduce

```
export PATH=$HOME/.cargo/bin:$PATH
cargo test -p graftyard-git --test prsync
python3 tests/m2/bf24/plant.py                      # every plant caught, exit 0
node workers/site/deploy.mjs tests/m1/wp1/sites.json --genesis tests/m1/wp7/genesis.json --test-faults --image <digest>
python3 tests/m2/bf24/bf24.py dist 75 --conflict-every 3
python3 tests/m2/bf24/bf24.py lc 30 --conflict-every 4 --transfer-at 2,5,9,13,17,21,25 --lose-at 7,11,15,19,23,27 --reset-at 9,17,25
python3 tests/m2/bf24/sum.py tests/m2/results/raw/wp5/dist.jsonl
python3 tests/m2/bf24/audit.py tests/m2/results/raw/wp5/dist.jsonl
```

The driver resumes after its last JSON line. It needs `~/.graft-integ/repos.json` and `tokens.txt` (the `wp6/bf24` writer token) and `tests/m0/.m0-token` for the member calls through the M1-WP7 test edge.

## 2026-11-05: the in-band first attempt, and BF-24 rerun on `graft-m2b`

Run on 2026-11-05 against the second test install `graft-m2b-site-a/b/c` (enam, weur, apac) and `graft-m2b-edge` (`tests/m2/results/install-m2b.md`), on branch `bf24-direct`. The repository is `m2b/bf24` (`repo:65538`), seeded with `tests/m1/integ/seed.ts`, with pull request #1 from `refs/heads/pr` to `main`. The queues `graft-m2b-prsync-a/b/c` were created for it. `graft-m1-*` was not touched. Raw data is in `raw/bf24/`.

### The decision, and why it held

The operator's decision is in `decisions/DECISIONS.md`, "`PrSync`'s first attempt runs in band, and Queues carries only the re-send". It held. No flip condition was met:

- **p99 within 10 s.** It held on both builds, at 5.54 s and 6.44 s (below).
- **No sign that the in-band merge slows the leader.** The first attempt is spawned from the alarm, with a `FIRST_FOR` bound of 25 s. The alarm does not wait for it, and its commits go through the normal path. Over `dist2`'s 75 trials, the leader's `/stats` (`raw/bf24/dist2.stats.jsonl`) showed:
  - `commitMs` with n 151: p50 1,921 ms, p90 2,223 ms, p99 2,474 ms, max 3,057 ms. This is in line with M1-WP8's steady `Push` commits of 2.1 to 2.8 s at p50.
  - `roundMs` p50 183 ms and p99 209 ms.
  - `prsyncFirst` 75 and `prsyncSent` 0.
- **Queues is not needed for the head push.** No queue delivery was needed, so this flip does not apply.

How it is built (`12ec22a`, `76abf81`):

- `Dispatcher::batch` returns `Delivery::First` or `Delivery::Again`.
- `First` runs `prsync::one` in the leader's isolate, through the member's own bindings. That is the same handler the queue consumer runs, and it calls no Cloudflare API.
- `Again` covers work the committed cursor has not passed `REDELIVER_AFTER_MS` (30 s) after it was handed out. It goes to the site's queue, and so does a failed first attempt.
- One first attempt runs at a time per leadership. Later pushes wait for it and go out as one batch.

Correctness does not depend on the first attempt. Duplicates commit nothing, through the content-derived request ids and `decide_sync`'s checks. A first attempt lost with its isolate or leader is covered by the cursor and the re-send. The leader-change and purge runs below show both.

### BF-24 table

Latency runs from the return of `git push` to the first `ls-remote` that shows the result. "Server" runs on the leader's clock, from the push's entry to the `TestMerged` entry.

| Run, build | Trials | n | Within 10 s | p50 | p90 | p99 | Max | Server p50 / p99 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `dist`, `12ec22a` | Clean | 50 | 50 | 5.08 s | 5.34 s | 5.54 s | 5.54 s | 2.74 / 3.13 s |
| | Conflict | 25 | 25 | 5.06 s | 5.19 s | 5.31 s | 5.31 s | 2.73 / 2.90 s |
| `dist2`, `ca0d181` (final) | Clean | 50 | 50 | 5.50 s | 5.86 s | 6.44 s | 6.44 s | 3.18 / 3.45 s |
| | Conflict | 25 | 25 | 5.49 s | 5.68 s | 5.81 s | 5.81 s | 2.99 / 3.25 s |

- Every clean trial's merge commit had parents (base tip, head).
- `mergeable` matched each time: true for clean heads, false for conflicts.
- Every conflict deleted `refs/pull/1/merge`.
- The leader was site 0 (enam) throughout both runs.
- All 75 consumer lines of each run came from the in-band path, and none from the queue (`dist.consumer.jsonl`, `dist2.consumer.jsonl`).

**Verdict: pass.** p99 is 6.44 s on the final build, against the 10 s gate. The earlier queue-only run had a p99 over 30 s.

The log audit (`audit.py`) passes over all four runs together, 194 heads across entries 19 to 479 (`raw/bf24/all.audit.txt`):

- every head has exactly one `TestMerged` with the right outcome;
- `refs/pull/1/head` moved to each head once;
- the cursor never went back.

### Where the time goes now

From the consumer's log lines matched to `dist2`'s trials (p50, then p90):

| Step | p50 | p90 |
| --- | --- | --- |
| Push entry applied to handler start (alarm at 50 ms, scan, spawn) | 226 ms | 253 ms |
| Record lookup | 0 ms (from the member's join) | 0 ms |
| Fresh read of the pull request | 372 ms | 396 ms |
| `/merge` on the enam git cache shard | 2,160 ms | 2,379 ms |
| The `PrSync` commit | 583 ms | 612 ms |
| Handler total | 3,138 ms | 3,318 ms |
| Client gap after the server time (0.5 s polls of a 5.0 s `ls-remote`) | 2,388 ms | 2,645 ms |

- The git cache's `/merge` is now the largest server step, at about 70% of it.
- The queue hop of p50 2.0 s and p90 5.1 s, with its 37 s outliers, is gone from the head push.
- The rest of the client latency is the `ls-remote` through the edge. The latency starts after `git push` returns, which itself took p50 9.7 s.
- `dist` (`12ec22a`) split the same way, with a merge of 1,869 ms and a commit of 506 ms. `dist2` ran about 0.4 s slower, after `main`'s edge-wiring merge. Most of the difference is in the merge (+290 ms). Its cause was not established.

### Leader change, lost first attempts, and purged re-sends

Run `lc` (`raw/bf24/lc.jsonl`) on the final build, 40 trials, every fourth a conflict, with `BF24_DEADLINE` 60 s:

| Fault | Trials | Latency | Server | What covered it |
| --- | --- | --- | --- | --- |
| Transfer right after the push | 2, 5, 13, 21 | 6.60 to 6.94 s | 3.6 to 4.6 s | The new leader's dispatcher, from the committed cursor |
| Transfer, then `/reset` of the old leader | 9, 17, 25 | 6.29 to 7.18 s | 4.4 to 6.4 s | The same; at 9 and 17 the reset member led again later |
| Queue paused and purged at about 7 s, then transfer | 7, 11, 15, 19 | 14.74 to 15.30 s | 3.7 to 4.4 s | The first attempt, in band. Purging the queue removed nothing it needed. The client time is the harness's pause, purge, and transfer |
| First attempt dropped (`prsyncDrop`) | 27, 29, 31, 33 | 34.69 to 38.18 s | 34.0 to 37.7 s | The 30 s re-send through `graft-m2b-prsync-a`, queued 0.9 to 4.5 s |
| | 23 | 13.04 s | 12.7 s | Leadership moved from site b to site a without a transfer the driver injected. The new leader's first attempt covered it |
| Queue paused, then purged after the first re-send, with no first attempt dropped | 35, 37 | 5.21 to 5.91 s | 2.9 to 3.3 s | The in-band first attempt |
| No fault | 22 trials | p50 5.66 s, max 6.74 s | 2.9 to 5.6 s | In band |

`lc`'s purge trials could not reach the second re-send, because the in-band first attempt finished first. Run `purge` (`raw/bf24/purge.jsonl`) combines both faults in each of its 4 trials. It drops the first attempt, pauses the leader's queue, and purges it 42.6 to 43.0 s after the push, which is after the first re-send. All 4 synced at 67.1 to 69.3 s (server 66.1 to 68.8 s), from the second re-send at 60 s, through the queue. That includes the conflict trial, with `mergeable` false and the ref deleted.

`audit.py` passes on `lc` (40 heads, entries 213 to 316) and on `purge` (4 heads). Workers Logs show each dropped first attempt (`first dropped (test fault)`) and no second handling of any batch.

### The reset bug the leader-change run found

The first `lc` run, on `f3bac15` (`raw/bf24/lc-buggy.jsonl`, 13 trials), had trial 9 at 36.4 s. Its sequence:

- leadership was transferred, then the old leader on site c was reset at 12:39:43Z;
- the member restored and regained leadership in the same term;
- its dispatcher state is thread-local, keyed by DO name, and it outlived the reset;
- the "first attempt running" flag was keyed by term, so the restored member waited on an attempt that its old self had started and that would never return;
- Workers Logs show no first attempt after the reset, and the queue's re-send handled `193..197` at 12:40:18Z.

`76abf81` fixes it in two places:

- The dispatcher owns `running`. `finished` clears it, and a run older than `REDELIVER_AFTER_MS` counts as lost.
- The site keys dispatchers by `(term, boot)`, so a reset member starts a fresh one.

On the final build, trials 9, 17, and 25 took 6.3 to 7.2 s. New test: `a_first_attempt_lost_with_its_isolate_does_not_hold_later_pushes`.

### The `tier.rs` panic

F47's subagent saw `TierSql::get` panic on `expect("a fold read")` after "Network connection lost" (`workers/site/src/tier.rs`, then line 280). A panic rebuilds the wasm instance, which resets every member in the isolate.

`131ee88` makes a refused fold read, scan, row, or clear set the tier's `lost`, log `fold storage lost: …`, and answer no rows. The host checks `Family::lost` at these points:

- after each input's step;
- before a read resolution is answered;
- before a member reply;
- before the `PrSync` scan's batch;
- before a snapshot part uploads.

On a hit, `Host::fail_lost` resets only that object with `ctx.abort()`, as a failed write does. Test installs fail the core tier's next stored read with `/bench_seed {"failRead": true}`.

Verified on the final build (`raw/bf24/tier-failread.log`) on `repo:65537`'s site b member at 15:32:43Z:

- `/fold` answered 503 "member reset: fold storage lost";
- Workers Logs show `fold storage lost: a fold read of c__fold: Error: no such table: graftyard_no_such_table`;
- only `repo:65537` got a new boot id, while `repo:65536` and `repo:65538` in the same isolate kept theirs.

A first try at 15:31:32Z armed the fault, then sent a `/read` that the follower answered without a stored read. The fault was still armed when the second try ran.

### Planted bugs

`tests/m2/bf24/plant.py` on `ca0d181` (`raw/bf24/plant.log`): **14 of 14 caught**, exit 0. They are the 11 from M2-WP5 (0 to 10 in the table above) and 3 for the in-band path:

| Plant | Caught by |
| --- | --- |
| 11. Every first attempt restarts the redelivery clock, so a first attempt that keeps returning without passing the cursor starves the re-send | `first_attempts_that_return_without_passing_the_cursor_do_not_hold_back_the_re_send` |
| 12. A re-send goes in band again, not through the queue | 4 tests, including `a_first_attempt_runs_in_band_and_only_a_re_send_goes_to_the_queue` |
| 13. A first attempt lost with its isolate never counts as lost, so it holds every later push | `a_first_attempt_lost_with_its_isolate_does_not_hold_later_pushes` |

After `76abf81`, plant 11 was at first caught by nothing. The test that caught it, `a_first_attempt_that_never_returns_is_re_sent_while_pushes_keep_coming`, now covers a first attempt that never returns, so later pushes wait behind it. The new test covers attempts that do return, and catches the plant again.

### Checks

On `ca0d181`, `main` `31c1fd7` merged in (`raw/bf24/checks.log`):

- `cargo fmt --check`, ok.
- `cargo clippy --workspace --all-targets -- -D warnings`, ok.
- wasm32 clippy for site and edge, with and without `test-faults`: all 4 ok.
- `cargo test --workspace`: 42 suites, all ok.
- `go test` in `containers/git/server`, ok.
- Release-build greps: site 0 of 50 needles, edge 0 of 20.
- `crates/raft` is unchanged, so the long sweep and Porcupine were not rerun.

### Commits on `bf24-direct`

- `131ee88` site: a fold read storage refuses resets the member, not the isolate.
- `12ec22a` PrSync: the first attempt runs in band; the queue carries only re-sends.
- `c142f54` tests/m2/bf24: the install from the environment, and first-attempt log lines.
- `f3bac15` site: test installs drop a leader's next first attempt (`prsyncDrop`).
- `05faf7c` tests/m2/bf24: purge the re-send, a longer poll, and an audit that elects.
- `76abf81` PrSync: a reset leader waits on no first attempt of its old self.
- `ca0d181` Merge `main` (`31c1fd7`) into `bf24-direct`.

### What is deployed on `graft-m2b`

- `graft-m2b-site-a/b/c` run the `test-faults` build of `ca0d181` (`raw/bf24/deploy-site4.log`), versions `06e8ea70`, `db8a7f31`, and `ff84c2b8`, with image `sha256:5be873a0...`.
- Queues `graft-m2b-prsync-a/b/c`, each bound to its site as producer and consumer, with the M2-WP5 settings.
- `graft-m2b-edge` (`115bfa6a`) and `graft-m2b-tools-edge` (`a6c76344`) were not redeployed.
- `repo:65538` (`m2b/bf24`) holds pull request #1 and every trial's commits.

### Aborts seen

Workers Logs on `graft-m2b-site-a/b/c` from 11:00Z to 15:45Z (`raw/bf24/logs-sweep.txt`) have no `unreachable`, `panicked`, `Critical`, or `commit_tx` message. The only `fold storage lost` lines are the planted `failRead` tests on site b, at 11:35Z, 11:36Z, and 15:32Z.

### Design sentences this contradicts

Quoted, not edited:

- DESIGN, "Dominant access patterns": "*Webhook.* Leader alarm, cursor, Queue message, consumer Worker, POST." For `PrSync` the first attempt is now leader alarm, cursor, in-isolate handler, commit. The Queue message is only the re-send.
- SHARED, "Event fan-out": "`design/DESIGN.md` delivers events from leader cursors through Queues to consumer Workers". This no longer holds for `PrSync`'s first attempt.
- DESIGN, Failure modes, `event pipeline` instance: "(decided 2026-11-05, pending BF-24's rerun; `decisions/DECISIONS.md`)". The rerun is in, and it passes.
- DECISIONS, the in-band entry: "This is being built and measured now, so BF-24's 10 s clause stays failed until its rerun", and "The rerun is pending."
- BUILD-PLAN, M2-WP5 BF-24 status: "fails the 10 s clause" and "Decided and pending measurement".
- SHARED, "Issues and pull requests": "and the 4 of 50 BF-24 trials held about 37 s in Queues". With the in-band attempt, the head ref now waits only on the merge.
- DESIGN, `event pipeline` region row: "One site's queue and consumers … `PrSync` for that site's leaders". A lost queue now delays only re-sends. A lost git cache shard still holds both refs.
- Not covered by any row: the in-band first attempt runs in the leader's isolate, so a slow `/merge` holds one isolate's work for up to `FIRST_FOR` (25 s). DESIGN has no Failure-modes row for that.

### Reproduce

```
. <env with CLOUDFLARE_*, GRAFT_EDGE, GRAFT_TOOLS_EDGE, BF20_HOME=~/.graft-m2b, BF24_OWNER=m2b, BF24_REPO=bf24, BF24_PREFIX=graft-m2b, BF24_RAW>
python3 tests/m2/bf24/plant.py
python3 tests/m2/bf24/bf24.py dist2 75 --conflict-every 3
BF24_DEADLINE=60 python3 tests/m2/bf24/bf24.py lc 40 --conflict-every 4 --transfer-at 2,5,9,13,17,21,25 --lose-at 7,11,15,19 --reset-at 9,17,25 --drop-first-at 23,27,29,31,33 --purge-resend-at 35,37
BF24_DEADLINE=100 python3 tests/m2/bf24/bf24.py purge 4 --conflict-every 4 --drop-first-at 0,1,2,3 --purge-resend-at 0,1,2,3
python3 tests/m2/bf24/sum.py $BF24_RAW/dist2.jsonl
python3 tests/m2/bf24/audit.py $BF24_RAW/dist.jsonl $BF24_RAW/lc.jsonl $BF24_RAW/dist2.jsonl $BF24_RAW/purge.jsonl
```
