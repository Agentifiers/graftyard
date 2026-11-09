# AM1 fix: LM-1's two outright failures (L0-12's stall after a leader change, LF-12's commit p99)

These results are from 2026-11-09, on branch `am1-fix` (worktree `.worktrees/am1-fix`), cut from `main` at `6042f3e`. The test install is `graft-m1`. Nothing was pushed or merged. The raw logs are in `tests/am1/raw/` on the orb, which is git-ignored, so this file carries the numbers.

## Summary

| Failure | Root cause | Fix | Before | After |
| --- | --- | --- | --- | --- |
| L0-12: a stall ended by a leader change moved no lease | Only the old leader knew when the stall began. Followers recorded no marker, because the blackhole also blocked `suspect` reports. The winner's campaign marker was from about the time the fault cleared, so `by` was about 0 and its term-start noop had no `extend`. That noop's `at` was after the old leader's marker, so applying it cleared that marker everywhere (`member/log.rs`) | `crates/raft/src/member/follower.rs`: a follower that stores a re-send of an entry still above the commit, sent at least CALL_DEADLINE after the entry's `at`, marks `stalled_since` at that `at`. This is the condition `tick_leader` uses on the leader. The follower that wins the next term then stamps `extend` from it | In memory: the new gate fails (no `extend` on the new leader's first entry). Hosted (day 5): 30.2 min of stall, 20 leases moved 0 ms | In memory: `extend { since ≤ stall start, by ≥ 30 min }` on the first entry of the new term, and 3 of 3 leases moved by `by`. Hosted (day 7): 30-minute stall, then a leader change to site 1 (by `/transfer` from the old leader once site 1 held its re-send). The first entry of term 3 carried `extend { by: 1,761,355 ms }`, and all 20 leases moved by 1,761 or 1,762 s |
| LF-12: leader commit p99 3.4 s in the hold bursts | Not the leader's CPU, and not held polls in the leader's isolate (it holds none). The leader waited on a member call: its dep read of the owner's `Authz` chain. Every poll on a learner whose cached owner `Authz` was stale (the first poll, then once a minute) sent its own `/read` to the owner's `Authz` leader. Measured: 300 polls on a stale learner sent 300 reads. When 5,000 polls landed on 11 new learners at once (phase C), the repository leader's dep read of that owner came back 6.3 s late | `workers/site/src/feed.rs`: one owner read per member at a time. The first poll reads, and the polls that arrive meanwhile wait for that read (`Feed::refresh`, `Refreshed`) | Phase C on lf1-4: leader `commitMs` p99 **6,481 ms**, and one dep read of `authz:7100001` took 6,330 ms. 3,579 of 5,000 polls failed (`status null`: no answer from the member) | Phase C on lf1-6: leader `commitMs` p99 **474 ms** (n 50). 5,000 of 5,000 polls held, 0 failed. Target ≤ 600 ms met on the member's own number. The client p99 through the edge is above 600 ms in every phase, polls or none (see below) |

## L0-12: the stall after a leader change

### Root cause, from the code and the hosted trace

- `mark_stall` (`member/election.rs`) records the earliest marker. A follower's `suspect` report and a real campaign start call it, and `tick_leader` (`member/leader.rs`) marks the leader's own unacknowledged entry.
- With every member call blackholed, no `suspect` report reaches a follower (they are member calls too), and no campaign can win. So during the stall only the old leader holds a marker: site 0 at term 2, `stalledSince` = the `at` of its in-flight entry 4327 (`tests/am1/results/am1-hosted.md`, L0-12 stall).
- When calls work again, a follower campaigns (pre-vote first, then the real campaign in `check_campaign`), and `mark_stall` gives it a marker from about now.
- `become_leader` builds the term-start noop through `take_extend` (wired). `take_extend` computes `since = max(marker, last entry's at)` and `by = now − since`. With the marker at about now, `by` is about 0, and it returns `None`.
- Applying that noop (`at` after the old leader's marker) clears the old leader's marker too (`set_commit`'s clear rule). The stall is lost on every member. Hosted: noop 4328, no `extend`, all 20 leases moved 0.

The new leader's own lower bound was already right: the last entry it holds is entry 4327 (the re-send it stored), whose `at` is the stall's start. Only the marker was missing.

### The fix

A follower that accepts an AppendEntries whose entries include one of the leader's own term, above both its commit and the append's commit, sent (`req.sent_at`) at least CALL_DEADLINE after that entry's `at`, records `stalled_since = min(marker, that entry's at)`. That is the leader's own `tick_leader` condition, learned from the re-send that every working replication path sends after a missed CALL_DEADLINE. The follower writes it with its next state row, like every marker (`design/DESIGN.md`, Shape, `ExtendLeases`). The design's rules still hold:

- `since` is at least the `at` of the last entry the new leader holds. `take_extend` is unchanged, and the marker is never later than that entry's `at`.
- One stall moves a lease once. The winner's term-start entry carries `extend`, and committing it clears every member's marker (`set_commit`, unchanged). The gate counts exactly one `extend` after the stall.
- An idle group sets no marker. A prompt append (sent within CALL_DEADLINE of the entry's `at`) marks nothing, and neither does a delayed copy of an append the group has since committed past (scenario below).

### In-memory gates (real raft members)

- New: `crates/git/tests/lease_gates.rs`, `l0_12_a_stall_ended_by_a_leader_change_moves_every_live_lease`.
  - 3 Shared leases. The leader records an entry whose AppendEntries are lost, then 30 minutes pass.
  - The old leader's tick re-sends; both followers store it; the answers are lost. A `suspect` report reaches the follower that will win, before the re-send does, so it holds a later marker first.
  - After ELECTION_AFTER, that follower campaigns and wins term 3.
  - It asserts: the new leader's first entry committed and carries `extend` with `since ≤ stall start` and `by ≥ 30 min`; the next command (a renew) succeeds; every lease is live with `expires` moved by exactly `by`; exactly one entry after the stall carries `extend`; every member's marker clears.
  - On `6042f3e` it fails: "the new leader's first entry carries no extend: LogEntry { term: Term(3), index: Index(11), … extend: None, body: Noop }". With the fix it passes.
- New: `crates/raft/tests/scenarios.rs`, `a_follower_marks_a_stall_only_from_a_late_resend_of_an_uncommitted_entry`. A prompt append marks nothing. The re-send after CALL_DEADLINE marks the entry's `at`. A delayed copy of that re-send, delivered after the group committed past it, marks nothing.
- Changed: the same-leader gate (`l0_12_a_stall_moves_every_live_lease_by_its_length`) used to assert every member's marker cleared right after the extending commit. With the fix, the followers also mark the stall from the re-send, and a follower clears its marker when it applies the extending entry, which it learns from the next append. The helper `assert_markers_clear` now checks the leader at once and every member after one more entry. Leases, `by`, and the one-`extend` count are unchanged.

A path the fix does not cover: the old leader never reaches the winner again after CALL_DEADLINE, because it is down or still cut off when the other two can talk. The winner then has no re-send to learn from, and its term-start entry carries no `extend`. A temporary in-memory probe of that path showed `extend: None`, the old behavior, and was deleted. The winner cannot infer the stall from its own log: an uncommitted last entry on a follower also looks like an idle group's last write, which followers hear committed only with the next append. Only the old leader held the marker, so this is the same loss the design accepts for a marker that a crash loses before its state write ("a crash before then loses the stall, and leases lapse by it"). The hosted stall below ends with the old leader alive, which is the covered path.

### Long sweep and Porcupine (after the raft change, `96dc541`)

- `RAFT_SEEDS=2000 RAFT_STEPS=10000 RAFT_HISTORY=… cargo test -p graftyard-raft --release --test sim -- --ignored long_sweep`: ok in 335.75 s (359 s with the build).
  - HARSH: 20,109,327 steps, 160,083 ops (49,655 acked, 18,657 refused, 60,702 reads, 31,069 no reply), 226,062 crashes (97,860 mid-input), 3,525 leaders, 0 step-downs.
  - GRAY: 20,082,389 steps, 2,226,532 ops, 1,534 `STEP_DOWN` transfers, 17,109 gray faults.
- Porcupine (`tests/m0/porcupine`, whole history): HARSH `{"droppedFailedReads":3341,"noReplyWrites":27728,"operations":156742,"result":"Ok"}`, GRAY `{"droppedFailedReads":11,"noReplyWrites":209,"operations":2226521,"result":"Ok"}`.

### Hosted: the 30-minute stall with a leader change

`STALL_REPO=lf1-5 STALL_REPO_ID=1048582 node tests/am1/hosted/stall.mjs 30 20 change` on site day 7 (`raw/stall-change.log`, `raw/stall.jsonl`). It ran on lf1-5 because j4 no longer restores (below).

- 20 leases with a TTL of 3,600 s (10 Exclusive on distinct tasks, 10 Shared). The every-site fault (`/_bed/fault?site=255`) was armed for 03:57:01.086Z to 04:27:01.086Z, with task writes every 250 ms across the cutover.
- During the stall: the leader (site 0, term 2) held entry 71 in flight and recorded `stalledSince` = 1794196661447 (03:57:41.447Z, entry 71's `at`). The followers recorded none, as on day 5.
- After the fault lapsed, the old leader re-sent entry 71, and both followers stored it and committed through 70. The minute-30 status, taken then, shows `stalledSince` = 1794196661447 on all three members: the two followers learned it from the re-send. That is the fix at work. On day 5 the followers held none at the same point.
- No election started before then, because the old leader was alive and renewing. The driver had site 0 transfer to site 1 (`/transfer`, `Ok`), and site 1 led term 3. A transfer's campaign records no marker (`start_campaign` skips `mark_stall` for a transfer), so site 1's marker could only have come from the re-send. Day 5's leader change was an election (site 2 won term 3); that path uses the same marker and is the in-memory gate's scenario.
- Term 3's first entry: noop 72, `at` 1794198422802 (04:27:02.802Z), `extend { since: 1794196661447, by: 1,761,355 ms }`. Exactly one entry carries `extend`: entries 68 to 71 (term 2) and 73 have none.
- After the stall: 20 of 20 leases live. For every lease, `expires_at − granted_at − 3,600 s` = 1,761 s or 1,762 s, which is `by` rounded to the API's whole seconds. Every lease moved by the recorded stall, against 0 on day 5.
- `by` runs from the in-flight entry's `at` (03:57:41) to the new term's first entry (04:27:02). The every-site fault began at 03:57:01. Nothing was in flight in the 40 s before the marker: the leader had committed through 70, and entry 71, the one cutover write that reached `decide` during the fault, was appended 40 s in. The design counts no idle time, so `by` is 1,761 s rather than the fault's 1,800 s.
- Every marker cleared on every member after the first write in term 3.
- Member clocks against each other: spread at most 65 ms before and 55 ms after, with half round trips at most 136 ms. (The absolute offset of about 6.3 s is this orb's own clock, as on day 5.)
- Cutover writes: 6 × 201, 14 × 500, 4 × 503. The first write after the stall got 201. An idle 60 s Exclusive (201), then 10 minutes later an overlapping Exclusive from another task: 201 with 0 conflicts.

The driver read the lease API's `expires`, but the API's field is `expires_at`, so its `moved` column read null for all 20 leases, on day 5 (the main checkout's `tests/am1/raw/stall.jsonl`) and in this run's first readout. Day 5's "moved by 0" was read by hand from `expires_at − granted_at` = 3,600 s exactly, and its `/log` showed no `extend` on noop 4328, so that finding stands. The driver now computes `expires_at − granted_at − TTL` (`2e7447c`). This run's numbers above come from a one-off read of the same list with that formula.

## LF-12: commit p99 in the hold bursts

### What the number was

The 3.4 s from am1-hosted was the leader member's `commitMs` (admission to resolution, `workers/site/src/host.rs`), with the misses clustered in the hold bursts. I reran the row on fresh groups with drivers that read the members' own timing:

- `lf12why.mjs`: phase timing, with every voter's `/stats` reset before the phase.
- `lf12watch.mjs`: every voter's `/stats` and `/status` every 2 s as a time series.
- `lf12herd.mjs`: bursts of polls on learners whose owner was stale and then fresh, with each entry's dep reads from the leader's test-faults `/deps_log` and the `/read`s the owner's `Authz` members took.

### Which of the three it was

| Hypothesis | Evidence | Verdict |
| --- | --- | --- |
| Leader CPU-bound | Leader `applySteps` (synchronous ms per input that applied entries) were 0 ms at p99 in every phase. Storage `confirmMs` p99 was 21 to 71 ms. `roundMs` (append to apply) p99 was 136 to 178 ms throughout | No |
| Blocked behind held polls in the same isolate | The leader holds none (`leaderPolls` 0; the edge skips the leading voter). Holding 2,000 polls on the followers, or 5,000 on the learners from a steady state, left the leader's commit p99 at 430 to 507 ms, against 513 ms with no polls (lf1-0) | No |
| Waiting on member calls | Phase C on lf1-4 (before the fix): every slow commit had a dep read. Entry 101 waited 6,330 ms on `authz:7100001` while `tokens` and `authz:1` answered in 43 and 70 ms. The day 5 run's leader on lf1-0 shows the same pattern in `/deps_log`: 2,702 ms and 1,504 ms on `authz:7100001` at 20:31:10 to 20:31:14Z, the second phase C. The owner's `Authz` leader took one `/read` per poll: 300 polls on one stale learner gave 300 reads. A 5,000-poll burst spread over 11 learners gave 631, because most polls queued behind the first poll's read in the same object and found the owner fresh | **Yes**: the leader's dep read of the owner's `Authz`, slowed by the feed's own owner refreshes on the learners |

Why the burst on fresh learners was the worst case: a new learner's feed has no owner yet (`owner_at` 0), so every one of its first polls is stale. Phase C sent 5,000 polls to 11 new learners within 3 s, so each stale learner's polls sent their reads to `authz:7100001`'s leader at once, while the repository leader's commits needed a dep read from the same leader. A steady-state burst (owner refreshed in the last minute) sent fewer, and the herd runs show it:

| Run (lf1-4, 11 learners) | Owner `/read`s taken by its leader | Leader `commitMs` p99 |
| --- | --- | --- |
| 3,000 polls spread over 300 ms, stale owner, before the fix | 263 | 490 ms (whole 50 s run) |
| 5,000 polls at once, stale owner, before the fix | 631 | 589 ms |
| 5,000 polls at once, fresh owner, before the fix | 178 | (same run) |
| 5,000 polls at once, stale owner, after the fix | 252 | 496 ms |
| 5,000 polls at once, fresh owner, after the fix | 11 | (same run) |

With the fix, a stale burst costs each learner one owner read instead of one per poll. The other `/read`s the owner's leader takes in a window come from the edge, which reads the owner's `Authz` through `fresh` for every request it serves, the commit PATCHes included (`workers/edge/src/api.rs`, `read`). The fix does not touch those. In the fresh burst after the fix the count was 11. The herd runs did not themselves reach the 6 s tail, because a spread-out burst on a steady group gives the owner's leader time to drain. The tail shows up in phase C right after `place_learners`, when every learner is new.

### After the fix: LF-12 rerun hosted (`feed.mjs lf12` on lf1-6, fresh group, day 7)

| Phase | Before (lf1-4, day 6 code `04e4f22`, 03:02Z) | After (lf1-6, day 7 code `c7384ec`, 03:48Z) |
| --- | --- | --- |
| A, 400 polls through the edge from one IP | 40 × 200, 360 × 429 with `Retry-After` | 40 × 200, 360 × 429 with `Retry-After` |
| B, 5,000 polls on the 2 followers | 1,000 held each, 3,000 `full`; leader `commitMs` p99 665 ms (n 50) | 1,000 held each, 3,000 `full`; leader `commitMs` p99 710 ms (n 50) |
| `place_learners` | 11 learners from peaks 1,000 and 1,000 and refusals 1,860 and 1,500 | 11 learners from the same peaks and refusals |
| C, 5,000 polls on the learners | 1,421 held, 3,579 failed (`status null`: no answer from the member); leader `commitMs` p99 **6,481 ms** (n 50) | 5,000 held (454 or 455 each), 0 failed; leader `commitMs` p99 **474 ms** (n 50), p50 217 ms |
| Polls on voters after placement | 0 more | 0 more |

Against the gate (`build/BUILD-PLAN.md`, LF-12: "Commit p99 at most 600 ms once `place_learners` has run and at most 1 s before it"):

- On the leader member's own `commitMs`, after placement: 474 ms, a pass. Before placement: 710 ms on the fix run and 665 ms on the run before it, a pass under the 1 s line.
- On the client's latency through the edge, the gate fails in every phase, with or without polls. With no polls at all (lf1-0, `lf12why base`), the client p50 is 994 ms and p99 2,708 ms, while the leader's `commitMs` p50 is 196 ms. The difference is outside the commit: the edge's request path from this orb (auth, the task API's reads, the hedge, the outcome read), about 800 ms at p50. LF-12 measured commit latency, so this file reports the member's number as the row's and the client's beside it. I did not tune the edge path; that would be a different row's work.
- Phase B's 710 ms is not caused by the polls. The followers holding 2,000 polls showed `commitMs` p99 430 to 507 ms in `lf12why held`, and the slowest samples in B come from the far follower's append round trip (site 2, `appendMs` p99 323 to 729 ms in the `lf12why` runs), as M0's LF-12 found ("the far follower's round trip").

## Task 3: a Graftyard-run App and an MCP App

Not run. It is not cheap on this install:

- `PutAgentApp` with `GraftyardRun` or `Mcp` must run on `Authz(Genesis.account)` (`install_account()`, account 1), and the App must be one the install owns, registered there (`AuthzErr::NotOwnApp`).
- m3-ci and the triage App are owned by m3-admin (7100003). An App the install owns needs its `PutApp` on `authz:1`, its `Names` rows (`RegisterApp`, `RegisterClient`, the slug), an installation on m3org with its own `RegisterInstallation`, and a key, as `tests/m3/hosted/triage.ts` does for an admin-owned App.
- Then `rows.mjs l012` needs a third token source for each kind, and it runs on j4, which no longer restores (below).

I judged that more than cheap and left it.

## A finding: main's 32.13 merge renumbered the stored lease events

After site day 7, j4 (`repo:1048576`), lf1-2 (`repo:1048579`) and lf1-3 (`repo:1048580`) do not restore on any member. Each reports `restoreError` "log row 3838 / 26 / 34: Serde Deserialization Error". These are the groups that hold lease entries.

- `RepoEvent` has two encodings. The digest encoding (`crates/git/src/repo.rs`, `tagged(out, …)`) uses explicit tags, and lease events are 35 to 38 both at `9fb5032` and at `6042f3e`. The stored and member-to-member encoding (`graftyard_raft::codec`, postcard through `#[derive(Serialize, Deserialize)]`) numbers variants by their position in the enum.
- At `9fb5032` (main after AM1-WP4) and on day 6 (`04e4f22`), `RepoEvent` had 37 variants, and `LeaseAcquire`, `LeaseRenew`, `LeaseRelease` and `PathSet` were serde variants 33 to 36.
- At `6042f3e`, the `m3-repos` merge (`3160687`) put `Created` and `Deleted` before them in the enum. They became serde variants 33 and 34, and the lease events moved to 35 to 38. A stored `LeaseAcquire` now decodes as `Created { actor }`, and the row fails.
- The groups with no lease entries (lf1-0, lf1-1, lf1-4 to lf1-8, and the Authz chains) restore fine.

This breaks more than this install. Any member that stored a lease event under `9fb5032` cannot restore under `6042f3e`. In a site day that rolls one site at a time, a member on the old release and one on the new would also misread each other's entries in AppendEntries. Fixing it means choosing which stored layout wins: lease events back at 33 to 36 (which breaks whatever stored 32.13 events under `m3-repos`, on graft-m2b), or `Created`/`Deleted` at 33 and 34 (which breaks lease events stored under `9fb5032`, like these three groups). An explicit serde variant index on each `RepoEvent` variant would stop it from happening again. Both are changes to main outside this task, so I made neither; they need a decision.

Here, it means LM-1's j4 rows (the L0-12 refusal counts, B0-36 and B0-37) and L0-16's lf1-2 cannot rerun on this install until those groups are recreated or the layout is fixed. Today's runs used lf1-4, lf1-5 and lf1-6.

## Deployed versions (site day 7)

`tests/am1/hosted/day7.sh` from `c7384ec` (the two fixes plus `/log`'s `extend`), the same shard image `sha256:5db19b4e…`, `noroll`.

| Day | Commit | Sites a / b / c / d | Edge | Ops |
| --- | --- | --- | --- | --- |
| 7 | `c7384ec` | `b5bfd297` / `a0ee9724` / `da589789` / `9199c4e4` | `382356ed` | unchanged (`1e9eb629`) |

The first pass of `deploy.mjs` (`raw/siteday7.log`) uploaded phase 1 for all four sites and phase 2 for a and b. Then wrangler failed on site c with "fetch failed" (a network error). The rerun with `--skip-build` (`raw/siteday7b.log`) used the same build and ended with exit 0. The table lists that run's second, current version of each site. The edge's `FORMATS_ENABLED` is unchanged: `32.1,32.4,32.5,32.6,32.7,32.9,16.2,17.2,18.2,20.1,32.10,18.4,32.8,32.12,32.14,18.3`.

No new record format was taken.

## Plants (`tests/am1/fix/plant.py`)

9 of 9 caught (`raw/plant-fix.log`).

| # | Mutant | Caught by |
| --- | --- | --- |
| 0 | a follower never marks the stall from a re-send (the bug as hosted) | `l0_12_a_stall_ended_by_a_leader_change_moves_every_live_lease` |
| 1 | a follower marks the re-send time, not the entry's `at` | same |
| 2 | a follower keeps its later marker over the entry's earlier `at` | same |
| 3 | a follower marks every re-sent entry, however fresh | `a_follower_marks_a_stall_only_from_a_late_resend_of_an_uncommitted_entry` |
| 4 | a follower marks entries the group already committed | same |
| 5 | every poll on a stale owner reads it (the herd as hosted) | `feed::tests::a_burst_of_polls_on_a_stale_owner_sends_one_read` |
| 6 | the waiting polls are never woken | same |
| 7 | a finished read stays in flight | same |
| 8 | a fresh owner is read again | same |

## Checks

`/tmp/am1fix/checks.sh` on `55f5854`, all exit 0:

- `cargo fmt --all --check`.
- `cargo clippy --workspace --all-targets -D warnings`.
- wasm32 clippy for site and edge, each with and without `test-faults`, and for ops.
- `cargo test --workspace`: 576 passed, 0 failed.
- `scripts/check_design.py`, `check_names.py`, `check_links.py`.
- `go test ./...` in `containers/git/server`.

Token scan of `tests/am1/raw` and `/tmp/am1fix` (41 files): 0 matches for `gh[pousr]_…`, `github_pat_…`, or a JWT, and 0 hits for the 4 known secrets (the bed and tools tokens and the two m3 tokens).

## Commits (on `am1-fix`, oldest first)

- `96dc541` raft: a follower marks the stall from the leader's re-sent unacknowledged entry (L0-12), with the new lease gate.
- `760f9c0` feed: one owner `Authz` read per member at a time (LF-12), with the drivers `lf12why`, `lf12watch` and `lf12herd`.
- `1117d4b` site: test-faults `/log` shows each entry's `extend`.
- `c7384ec` site day 7 script (the deployed commit).
- `55f5854` raft scenario for the follower's marker; the lease gate's winner gets a `suspect` report before the re-send; the stall driver's leader change; plants.
- `c9ab1fb` the stall driver takes `STALL_REPO`.
- `b7db630` results draft.
- `2e7447c` the stall driver reads `expires_at` and `granted_at`.
- This file.

`crates/raft` changed only in `96dc541`. The sweep and Porcupine above ran on it, and the commits after it change no raft source.

## Design contradictions

1. **The stall marker's sources do not cover a blackholed stall that ends in a leader change.** `design/DESIGN.md`, Shape, `ExtendLeases`: "A member marks `stalled_since` at the earliest of a follower's first `suspect` report, a candidate's campaign start, or the leader's first entry that no quorum acknowledged within CALL_DEADLINE". With every member call failing, the first two come only after the fault clears, and the third lives on the old leader alone. The fix adds a fourth source, a follower's re-send of that same entry, and it is the one that carries the stall to the next leader. `design/AGENT-LAYER.md` (A8 gray incident) says "The `suspect` reports set `stalled_since`, and the new term's first entry carries `ExtendLeases`, which moves every live lease by the stall". That holds only when reports reach followers during the stall, which a full blackhole prevents. The design text should name the follower's re-send marker; I did not edit it.
2. **LF-12's commit p99 does not say where it is measured.** `build/BUILD-PLAN.md`, LF-12: "Commit p99 at most 600 ms once `place_learners` has run and at most 1 s before it". The leader member's commit time passes. The client's latency through the edge fails in every phase, including with no polls (p50 994 ms, p99 2,708 ms against the member's 196 / 513 ms). M0's LF-12 measured commit latency at its load clients, which ran as Durable Object alarms on Cloudflare (`tests/m0/results/m0-wp12.md`, LF-12 `r1`: "each client's alarm is one invocation"), not from an orb.
