# b033-profile: the repository leader's CPU per command by stage, profiled locally

LM-1, 2026-11-22. The local profile asked for after the quarter-load set (`tests/am1/results/b033-quarter.md`):
the leader's work on `/commit`, learner `/raft` and `/entries`, by stage, at about 60 commands an entry (full load)
and about 11 (quarter load), with each cost marked per command, per entry or per call. $0, no hosted runs. No code
that cuts CPU is in this branch. Tags follow `design/SHARED.md`, "Read this first".

**Short answer.**
- **The local bed accounts for 20 to 46% of the hosted leader's CPU** [measured, both]. That is 0.9 to 2.1 ms of the
  hosted 4.5 to 4.9 ms per command cut. The rest happens outside the stages the bed runs; see "What the bed cannot see".
- **On a fresh fold, the leader's own per-entry work is small and mostly per command.** About 0.58 ms a command at 60
  an entry and 1.8 ms at 11 an entry. Every stage fits "a fixed part per entry plus a part per command" (table below)
  [measured].
- **One cost grows with the repository's open pull requests, not with load** [measured, local; hosted consistent]:
  `graftyard_fold::Read::prefix`'s first batch is 256 rows wide, and every `tiers::pulls_from(branch)` asks for 256
  rows of `pull_by_head` starting at that branch.
  - A pushed branch with no pull request of its own, as every agent branch is until its draft PR opens, then reads up to
    256 other pull requests' rows to learn it has none.
  - Each push pays this four times: once each in `apply` (`write_ref`), `prsync::touched`, `queue_consumer::touched`,
    and a 32.14 push's `PathSet`.
  - At 991 open pull requests, one call costs 1.3 ms against 0.11 ms at 91. Per entry of 60 (15 pushes), `fold_apply`
    grows from 8 to 31 ms, and the two consumer scans from 4 to 36 ms. That is 0.58 → 1.65 ms per command for the
    entry's own work.
  - Planted check: with the batch cut to 4 rows (local only, reverted), the growth disappears at 300 entries of state.
    `fold_apply` stays at 7 to 8 ms an entry and each consumer scan at 1 to 2 ms.
- **The hosted leader's `/commit` CPU per call rises through every run while the load stays flat**: 1.1 to 1.4 ms at
  30 s, 1.5 to 2.3 ms by 270 s on all six runs [measured, Workers Logs]. This fits the growth above, because draft PRs
  accumulate over the run [inference: a1 does not log how many PRs were open].
- **Where a cut pays.**
  - The `prefix` batch (or a point read for "does this branch have a pull request") removes a cost that grows with
    open pull requests and would otherwise rise without bound in a long run.
  - The other stages are each under 0.1 ms a command except `fold_apply` (99 µs a command plus 2.2 ms an entry).
  - The larger lever at a quarter load is outside the entry. Learner `/raft`, `/entries` and `/read` are 64% of the
    leader's CPU there (131 of 206 ms/s on qlC). The bed matches `/entries`, but it prices learner `/raft` at a tenth
    of its hosted cost; see "What the bed cannot see".

## Method

- **Bed:** `tests/leaderprof/run.sh` builds `tests/leaderprof/bed` with worker-build and runs it in local workerd
  (`wrangler dev` 4.153, workerd 2026-10-23) on this branch's base, main `c279386a`.
  - The bed compiles the member's own `workers/site/src/tier.rs` (tier tables, `seal`) and `txn.rs` (`commit_tx` in
    `transactionSync`).
  - It checks its member DDL against `workers/site/src/host.rs`.
  - It applies B0-33's committed mix: agent pushes 25%, task edits 30%, comments 12%, labels 8%, lease renews 14%,
    lease acquires 6%, draft PRs 5%.
  - Push bodies are real `RepoCmd::Push` wire bodies with 10 to 70 KB packs, inline up to 128 KiB an entry and stored
    after that.
  - Draft PRs open on agents' own pushed branches, as a1 opens them. Acquires reuse 100 lease ids, because a1 keeps at
    most about 100 live.
- **Timing:** each stage is timed with the isolate's clock, summed over 30 entries. workerd's `Date.now()` advances
  during synchronous CPU in local dev (a 438 ms busy loop read 438 ms), so stage times are CPU time.
  - Three runs on fresh objects (grow=0), three after 300 untimed entries of state (grow=300), and one with no peers.
  - Medians are below. Raw output: `tests/fanout/raw/leaderprof-c279386a.out`. Summary: `tests/leaderprof/table.py`.
- **Noise:** the orb ran another worktree's test jobs at a load average of 5 to 8 on 4 cores. Repeat runs moved single
  stages by up to about ±30%. Read cells to one figure.
- **Hosted:** `tests/leaderprof/hosted.py` on the leader's Workers Logs (`tests/fanout/raw/wl-bl`, `wl-ql`), 90 to
  300 s. Output: `tests/fanout/raw/leaderprof-hosted.out`.

## The leader's per-entry work, by stage, fresh fold

Medians of three runs, ms. "Unit" is what the stage is paid per. The fit uses the two shapes to split each stage into
a fixed part per entry and a part per command [measured; a two-point fit, so it is a split, not a model].

| Stage | Unit | N=60: ms/entry (ms/cmd) | N=11: ms/entry (ms/cmd) | Fit: ms per entry + µs per command |
|---|---|---|---|---|
| `/commit` body decode | command | 2.33 (0.039) | 1.40 (0.127) | 1.2 + 19 |
| fold apply (`apply_entry`) | entry | 8.13 (0.136) | 3.30 (0.300) | 2.2 + 99 |
| tier seal | entry | 0.17 | 0.03 | 0 + 3 |
| log row (encode, digest) | entry | 1.97 (0.033) | 1.60 (0.145) | 1.5 + 8 |
| request rows | command | 0.07 | 0.00 | 0 + 1 |
| SQL transaction (`commit_tx`) | entry | 2.97 (0.050) | 1.13 (0.103) | 0.7 + 38 |
| done reply render | command | 0.17 | 0.03 | 0 + 3 |
| PrSync scan (`prsync::touched`) | entry | 1.93 (0.032) | 0.33 (0.030) | 0 + 33 |
| merge-queue scan (`queue_consumer::touched`) | entry | 2.27 (0.038) | 0.30 (0.027) | 0 + 40 |
| AppendEntries encode, 6 peers | peer | 3.60 (0.060) | 2.73 (0.248) | 2.5 + 18 (0.42 + 3 a peer) |
| send's JS objects, 6 peers | peer | 0.50 | 0.57 | 0.6 + 0 (0.1 a peer) |
| learner `/raft` answer carrying the entry, 6 calls | call | 3.63 (0.060) | 2.67 (0.243) | 2.5 + 20 (0.41 + 3 a call) |
| learner `/raft` answer, empty, 6 calls | call | 0.07 | 0.03 | 0 |
| `/entries` page, 2 calls | call | 7.07 (0.118) | 5.60 (0.509) | 5.3 + 30 (2.6 + 15 a call) |
| **all stages above** | | **34.9 (0.58)** | **19.9 (1.81)** | |

- **Per command:** body decode, fold apply's share, the SQL transaction's bytes, and the two consumer scans. About
  0.24 ms a command in all.
- **Per entry:** fold apply's fixed part, the log row, the transaction, and the decode's fixed part. About 5.6 ms an
  entry. At 60 an entry that is 0.09 ms a command; at 11 it is 0.5.
- **Per call or member:** each peer send is about 0.5 ms (encode plus JS objects), each learner answer that carries
  the entry about 0.4 ms, and each `/entries` page about 2.6 ms plus 15 µs per command it lists. These do not shrink
  with commands, so at a quarter load they are most of the leader's per-entry work.
- workerd's process CPU (all threads, fixture building and the sink objects included) was 1.3 ms a command at 60 and
  3.5 at 11. So the stages above are 45 to 52% of what the bed's own process spent. The runtime floor was 1.3 to
  1.7 ms of process CPU per `/noop` request [measured].

## What grows: open pull requests

After 300 untimed entries the fold holds 991 open pull requests at N=60, and 183 at N=11 (a1's 5% draft PRs, never
closed in a run). It holds 19 lease rows.

| Stage at N=60 | grow=0 (91 open PRs) | grow=300 (991 open PRs) |
|---|---|---|
| fold apply, ms/entry | 8.13 | 31.33 |
| PrSync scan, ms/entry | 1.93 | 17.63 |
| merge-queue scan, ms/entry | 2.27 | 18.77 |
| one `pulls_from` for a branch with no PR that sorts among the PR heads, ms | 0.11 | 1.31 |
| the same for a branch that sorts after every PR head, ms | 0.05 | 0.08 |
| a 256-row fold scan, ms | 0.22 | 1.37 |
| a 1-row fold scan, ms | 0.05 | 0.07 |
| all stages, ms/command | 0.58 | 1.65 |

- **The cause is one read.** `pulls_from` reads `pull_by_head` with `Read::prefix(&[repo, branch])`
  (`crates/git/src/tiers.rs`). `prefix` asks `scan(table, from, 256)` and drops rows that fall outside the prefix only
  after reading them (`crates/fold/src/store.rs`). `TierSql::sql_scan` runs `WHERE (k0, k1) >= (?, ?) ORDER BY ...
  LIMIT 256` and decodes every row (`workers/site/src/tier.rs`).
  - So a branch with no pull request reads the next 256 rows of other branches' pull requests, while there are that
    many after it.
  - Agent branch names `claude/a1-<run>-<i>` sort among the open PRs' heads, so most pushes read the full 256.
  - A branch that sorts after every head reads none (0.08 ms).
- **Who calls it per push:** `RepoTiers::write_ref` in `apply`, `prsync::touched` and `queue_consumer::touched` on the
  leader's consumer scans, and `Push::path_sets` in `decide` on a 32.14 push. That is four 256-row reads per agent
  push once 256 PRs are open.
- **Plant:** the same bed with `prefix`'s batch cut to 4 rows (`crates/fold/src/store.rs`, local only, reverted before
  commit) gave, at grow=0 and grow=300, fold apply 7.5 and 8.0 ms an entry, PrSync scan 0.9 and 1.0, merge-queue scan
  1.3 and 1.4, and `pulls_from` 0.1 and 0.07 ms. Two runs each.
- **One kind alone does not grow**, since it is the mix that puts pushed branches among PR heads. With every command a
  push, or every command a draft PR on its own branch, fold apply stays flat across 300 entries. Pushes and draft PRs
  together grew 20 → 56 ms an entry.
- **Leases are not the growth here.** `lease::rows` (every lease row, which `LeaseView::live` reads, and which the
  feed ring reads once per applied entry on every member with an open feed) costs 0.1 to 0.2 ms at 19 rows. It costs
  4.5 ms at 1,117 rows (an earlier bed version that never reused lease ids) [measured]. a1 keeps about 100 live, so it
  matters only for a repository with many more live leases.
- **Hosted, consistent with it:** `/commit` CPU per call by 30 s bucket rose from 1.1 to 1.4 ms at 30 s to 1.5 to 2.3
  ms at 270 s on all six runs. Over the same window, `/commit` calls a second and commands cut stayed flat
  [measured, `hosted.py`]. a1 offers draft PRs at 0.42 a second per 500 agents (`tests/am1/rest/wire.mjs`) and
  never closes them in a run. That is about 1,100 offered by 270 s at full load (5,000 agents). At a quarter load
  it is about 280 offered; qlC's a1 opened 236 of 298 tried (`b033-quarter.md`). Both reach about 256, where a
  branch's read fills its batch [inference: a1 does not log how many opened on the full-load runs].

## Against the hosted leader

Hosted per-route CPU, 90 to 300 s [measured, `hosted.py`]:

| Run | Commands cut/s | Leader CPU ms/s | `/commit` | learner `/raft` | `/entries` | `/read` | `/outcome*` | Under-5-ms invocations |
|---|---|---|---|---|---|---|---|---|
| blB (EWR, full) | 118.3 | 533 | 279 (1.75 a call) | 87 (1.48) | 84 (4.76) | 15 | 45 | 62 (12%) |
| qlC (EWR, quarter) | 41.8 | 206 | 63 (1.43) | 76 (1.45) | 33 (3.96) | 22 | 0 | 53 (25%) |

The bed's stages, at the hosted rates (entries/s, 4 learners each taking the entry once, `/entries` calls/s)
[measured, local × hosted rates]:

| Run | Bed state | Commit path, bed / hosted `/commit` | Learner answers, bed / hosted learner `/raft` | `/entries`, bed / hosted | Bed total, ms per command (share of hosted) |
|---|---|---|---|---|---|
| blB | grow=0 | 42 / 279 | 5 / 87 | 62 / 84 | 0.92 (20%) |
| blB | grow=300 | 158 / 279 | 7 / 87 | 82 / 84 | 2.08 (46%) |
| qlC | grow=0 | 38 / 63 | 7 / 76 | 23 / 33 | 1.65 (33%) |
| qlC | grow=300 | 48 / 63 | 8 / 76 | 23 / 33 | 1.89 (38%) |

- **`/entries` matches.** The bed's page build is 74 to 97% of the hosted route's CPU.
- **`/commit` matches at full load only once the fold has grown**: 158 of 279 ms/s, against 42 on a fresh fold.
- **Learner `/raft` is a tenth or less of hosted.** The bed prices the answer only (a ReadIndex reply, and the held
  entry encoded into the response). Hosted spends about 1.45 ms a call over about 50 calls a second whatever the load:
  72 to 87 ms/s, the largest single route at a quarter load.

## What the bed cannot see

Each of these is on the hosted leader's CPU and not in the stages above. Their sizes are hosted measurements; what
fills them is [unverified]:

- **The runtime floor per invocation.** The hosted leader takes 3.0 invocations per command cut (122 to 351 a
  second). Invocations under 5 ms carry 51 to 62 ms/s on every run, at a mean of 0.18 ms (full) to 0.47 ms (quarter).
  That is 0.5 ms a command at full load and 1.1 to 1.3 at a quarter load [measured]. Local `/noop` was 1.3 to 1.7 ms
  of workerd process CPU per request, all threads.
- **Learner `/raft` beyond its answer.** Each call is a fetch into the leader. It is decoded, stepped through
  `Member`, and its response is built from held messages (`crate::callers`). The hedge path copies some calls.
- **Held-output release and `/outcome`.** Completing commit waiters, `outcome()` scans, and at full load 95
  `/outcome` calls a second at 0.37 ms each, plus `/outcome_by_request`.
- **`deps` reads.** Per entry, `fresh` reads the identity folds' chains. The native profile put deps_read at 1.1 ms an
  entry at a quarter load (`crates/git/tests/commit_profile.rs`); the bed does not run it.
- **`/read`.** 15 to 22 ms/s, at 2 to 7 ms a call.
- **Admission and the cut** (`commit_as`), and stats and trace rows.

## Native profile beside it

`SHAPE=b033 cargo test -p graftyard-git --release --test commit_profile -- --ignored --nocapture` (native x86, no
SQLite, no JS) gave 315 µs a command at 50 commands an entry and 781 µs at 10.5 [measured, 2026-11-22].
- Inline-pack body decode is 152 µs a command there, against the bed's 19 µs. The native harness prices admission as
  well (`/commit body decode + price`), so the two are not the same stage.
- The native per-peer call codec at 29 to 69 µs a call agrees with the bed's 0.4 to 0.5 ms a peer only within an order
  of magnitude. wasm under V8 is the hosted runtime, so use the bed's figure.

## What a cut would pay, from these numbers

Stated as arithmetic on the measurements above, not as a design [inference]:

| Cut | Full load, ms per command | Quarter load, ms per command |
|---|---|---|
| `pull_by_head` read bounded to the branch (or a point read), at about 1,000 open PRs | about 1.1 (bed grow=300 minus grow=0) | about 0.2 |
| Per-entry fixed costs halved (log row, transaction, decode fixed part) | 0.05 | 0.25 |
| `/entries` pages halved (fewer calls or a cheaper page) | 0.35 | 0.4 |
| Learner `/raft` halved (fewer calls, or less per call) | 0.37 | 0.9 |
| Per-invocation floor halved (fewer invocations per command) | 0.25 | 0.6 |

- At full load the open-PR read is the largest single item the bed can price, and it grows with run length.
- At a quarter load the per-call and per-invocation costs (learner `/raft`, `/entries`, small invocations) are most of
  the bill. The bed prices them at a fraction of hosted, so the next measurement is a hosted per-call breakdown of
  learner `/raft`, not more local stages.
