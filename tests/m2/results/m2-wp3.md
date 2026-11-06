# M2-WP3 hosted half: merges and the merge queue on `graft-m1-*`

Status: in progress. This file holds progress notes until the gates run.

## Progress log

- 2026-11-05. Branch `m2-wp3-hosted` from `d4025be`, with `main` `5e364cf` merged in as a fast-forward. Before any
  work, `wrangler deployments list` showed the edge-wiring versions still live: site a `4079ddd2`, b `9a84c900`,
  c `25231068`, and edge `06ed3ac3`. This branch had deployed nothing.
- `67610e8`. `MergeCmd` is hosted as `RepoCmd::Merge` (format 32.4). The container's `/merge` with `method` serves
  `GitCacheShard::merge` for merge, squash, and rebase chains. The edge's `Plan::Merge`, REST merge, and
  enqueue/dequeue are wired and gated on `FORMATS_ENABLED` containing 32.4. Local checks pass: `cargo fmt --check`,
  `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace` (42 suites, 0 failed), and
  `go test` in `containers/git/server`.
- The disk was full (0 bytes free on `/`). Removing this branch's own `target-wp3/debug/incremental` and the Docker
  build cache freed about 3 GB. Builds run with `CARGO_INCREMENTAL=0`.
- `e5ed40c`, `ce19408`. The site host (`workers/site/src/merge_queue.rs`), the `QueueView` read, per-base timers, the
  test seeds (`record`, `mqKill`, `mqHold`), the log audit fields, and the edge's `POST /statuses/{sha}`. Local checks
  pass again: 42 suites, 0 failed.
- Site day, step 1. The image was built from `containers/git` and pushed as `graft-m1-site-gitshard-git:wp3`, digest
  `sha256:9bf81a5eb7797d81f4af9598356a967cc83fcea4ee970f879a9b84dbe3ba96c8`. The first push hit "blob unknown to
  registry", as it did for m2-edge, and the retry succeeded. Logs are in `raw/m2-wp3/sd/`.
- Site day, step 2. `node workers/site/deploy.mjs tests/m1/wp1/sites.json --genesis tests/m1/wp7/genesis.json
  --test-faults --image <digest>` deployed the build of `ce19408` (site wasm sha256 `507126d0…27`).
- Site day, step 3. Two deploy passes left site a on `95edf2c1`, b on `c07c49a7`, and c on `7e1cb3c7`. The edge,
  `node tests/m1/integ/deploy-edge.mjs ... --test-faults --var FORMATS_ENABLED=32.1,32.4,32.5 --var LOGINS=...`, is
  on `3a048bed` (edge wasm sha256 `6982069a…50`).
- Site day, step 4. The running shard containers of `wp6/edge1` (196630) and `wp6/bf24` (196629) were torn down on
  all three sites with `/_bed/kill?repo=..&shard=0|1|2`, so they start again on the new image. Each answered
  `{"torn_down":true}`.
- `tests/m1/integ/seed.ts` (run from the main checkout, which holds the untracked `tests/m0/.m0-token`) created
  `wp6/wp3m` (196631) for direct merges and `wp6/wp3q` (196632) for the queue. It kept every existing repository and
  token.
- Smoke. `gh pr merge 1 -R wp6/wp3m --merge` (gh 2.102.1) moved `main` to `ce3c3b0f`, whose parents are the old base
  `74efdaa2` and the head `a6e16085`, committed by `graftyard-shard`. `gh pr view 1 --json state` answers `MERGED`.
  gh ran without `GH_DEBUG=api`, so this run's replies were not captured; the gate runs below capture every reply.
- `tests/m2/wp3/hosted.py` drives the hosted gates. `seed.ts` added `wp6/wp3c` (196633, for BF-23) and `wp6/wp3b`
  (196634, for BM-2). `hosted.py setup` recorded the settings (merge, squash, and rebase allowed) on `wp3m`, a rule
  requiring `ci` on `wp3c`, and the same rule with a `HeadGreen` queue (5/5/5, `min_wait_minutes` 0) on `wp3q` and
  `wp3b`.
- Found and fixed before any queue run: `dequeuePullRequest` read `pullRequestId`, but GHES 3.22's
  `DequeuePullRequestInput` names it `id` (`915036c`, with a test that fails without the fix). The edge needs a
  redeploy to carry it.
- Direct merges (`hosted.py direct d1` on `wp3m`) pass: `gh pr merge --merge` (parents base and head), `--squash`
  (one parent, the head's tree), and `--rebase` (two replayed commits on the base, the head's tree). Each exits 0 and
  `gh pr view` shows MERGED. REST `PUT /pulls/n/merge` answers 409 to a stale `sha`, 403 to the reader, 200 to the
  merge, and 405 to a second merge. 30 GraphQL and 8 REST replies validate with 0 failures. Two planted bad replies,
  `clientMutationId: 7` and `merged: "yes"`, both fail validation.
- BF-23 (`hosted.py bf23 bf23 100` on `wp3c`, 4 trials at a time) is running.
- The first queue trials on `wp3q` pass. Five PRs enqueued with `gh pr merge --merge` (gh sends
  `enablePullRequestAutoMerge`). The driver plays CI on each `gh-readonly-queue` tip and fails the 3rd. The log shows
  `GroupBuilt`, then `Ejected 3 CheckFailed`, a rebuild of the other four, and 4 single landings, each moving `main`
  from the previous tip. With a kill (`mqKill after_quorum`, down 25 s), the member's fault table records the kill
  at the landing's index on term 2, a new leader takes term 4, and the outcome is the same. `min_wait_minutes` is
  now 1, so the five build as one group, and CI reports the newest tip first.
- BF-21's 100 runs are split over `wp3q`, `wp3q2`, `wp3q3`, and `wp3q4` (196635-196637 from `seed.ts`), 25 each.
  The leader is killed at global trials 9, 19, ... 99, alternating `after_quorum` and `before_commit`.
- BM-2 passes 100 of 100 (`hosted.py bm2 bm2 100` on `wp3b`). Each trial sends three commands straight to the leader's
  `/commit`: a `Build` chained to a stale `parent_tip`, a `Build` whose chain breaks inside, and a `Land` whose
  second plan's `base_old` is stale. All 300 are refused `StaleChain`, and the built chain is unchanged after each
  trial.
- BF-1's first try (`wp3f`, 196638) failed for a known reason. The running site build is `ce19408`'s, whose test
  hold lets the next in-band attempt commit the held landing (entry 44, five `Merged` and `Landed`). The driver's
  copy was then refused `NotQueued`; the `PrSync` batch committed alone (entry 52). `29b2f16` and `fa13e9f` keep a
  held base idle until the driver releases it. BF-1 runs after a second site day carries them, once BF-21, BF-23,
  and BM-2 finish on this version.
- Validation so far: the gh captures of `b0`, `bf23`, `q0`, `qk`, `bf21-wp3q`, and `bf21-wp3q2` give 291 GraphQL and
  35 REST replies, 0 failed.
- BF-23 hosted, 100 trials on `wp3c` (`bf23.jsonl`, 34 merge, 33 squash, 33 rebase). In 100 of 100 trials, after
  `ci` success and then a newer pending `ci`:
  - `gh pr merge` refuses on BLOCKED.
  - The edge's own `mergePullRequest` is refused by `decide`. The message is "Required status check "ci" is in
    progress." in 90 trials, and "Base branch was modified" in 10 where a concurrent trial's merge moved `main`
    first.
  - `main` never takes the head's change, and the pull request stays OPEN.

  After a newer success, 99 trials merge. Trial 86's final `gh pr merge --rebase` got a 503 from the git cache:
  "Durable Object reset because its code was updated". No deploy ran then: `wrangler deployments list` still shows
  `95edf2c1`/`c07c49a7`/`7e1cb3c7`/`3a048bed`. The reset is Cloudflare's own, not a gate failure, because the merge
  is allowed after the new success and is not the stale-success merge BF-23 forbids. Rerun by hand, `gh pr merge
  89 --rebase` exited 0 and the PR is MERGED. 23 trials retried "Base branch was modified" up to 3 times; their
  4-way concurrency is the cause.
- Planted bugs rerun at `116e1ce` (`tests/m2/wp3/plant.py`, in a detached worktree so the live drivers were not
  touched): 14 of 14 caught. Plant 12 was re-aimed first, because `tiers()` now has a `MergeCmd::Cursor` arm and its
  old text no longer matched. It is the same bug: a landing tagged Core only.
- Local checks at `de87bf9`: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test
  -j 4 --workspace` (42 suites, 355 tests, 0 failed, exit 0), and `go test ./...` in `containers/git/server` all pass.
  `crates/raft` is untouched, so the long sweep and Porcupine did not run.
- The per-base timer (`hosted.py timer` on `wp3t`, 196639, `min_merge` 5, `min_wait_minutes` 1): two pull requests
  enqueued, then no entry until `GroupBuilt` 63.2 s and 63.1 s after the first `Enqueued` in the first two trials,
  and both land.
- The timer passes 5 of 5: `GroupBuilt` at 63.2, 63.1, 63.2, 63.1, and 63.2 s after the first `Enqueued`, with no
  queue, ref, or check entry between, and both pull requests merge each time.
- Validation across every gh capture so far (`d1`, `b0`, `bf23`, `q0`, `qk`, and the four `bf21-*` dirs, merged into
  `raw/m2-wp3/all-replies`): 477 GraphQL and 63 REST replies, 0 failed. That includes 50 `PullRequestAutoMerge`
  (gh's enqueue on a queued base), 10 `PullRequestMerge`, and the 4 REST merge replies (200, 403, 405, 409).
- BF-21 hosted, 100 runs over `wp3q`, `wp3q2`, `wp3q3`, and `wp3q4`, 25 each (`bf21-*.jsonl`). Each run:
  - opens 5 pull requests with `gh pr create` and enqueues all five with `gh pr merge --merge`;
  - plays CI on each queue tip and fails the 3rd pull request's tip and every tip after it.

  Results:
  - 99 of the 100 scheduled runs pass. Run 15 on `wp3q` stopped before any enqueue: gh's `pr create` hit "net/http:
    TLS handshake timeout" to the edge. Its four pull requests opened without being queued, and the log shows that
    run touched no queue entry and moved no base. One replacement run (`wp3q` trial 25) passes, so 100 completed
    runs pass.
  - In every completed run, the base audit over the leader's log passes (0 bad):
    - `main` moves only in entries whose events are `Merged`s followed by `Landed`, each move from the previous tip;
    - the failing pull request is ejected `CheckFailed { ci }` and stays OPEN and out of the queue;
    - the other four are MERGED.
  - 40 runs landed the four in one `QueueMerge` and 59 in two. Group sizes over all landings: 46 of 1, 26 of 2, 46
    of 3, 40 of 4. A run took 131 to 216 s, median 187 s.
  - Leader kills at global runs 9, 19, ... 99 (10 runs) alternate `after_quorum` and `before_commit`, down 25 s. Each
    member's fault table records the kill on a landing:
    - The 6 `after_quorum` kills fired at the `QueueMerge` entry itself (indexes 485, 174, 528, 307, 146, 470 hold
      `Merged`... `Landed`). It committed and stayed committed under the next leader.
    - The 4 `before_commit` kills fired before the landing's entry was written (the index the table records holds no
      events). The next leader's consumer landed the group again.

    In all 10, a new term was elected (terms 3 to 8), the landing was whole, and the audit passed.
- Second site day, from `9aeccf3`, for BF-1's hold fix (`29b2f16`, `fa13e9f`) and `dequeuePullRequest`'s `id`
  (`915036c`). `containers/` is unchanged since `ce19408`, so the image digest stays `sha256:9bf81a5e…c8`. Before:
  sites `95edf2c1`/`c07c49a7`/`7e1cb3c7`, edge `3a048bed`.
- Second site day done. `deploy.mjs ... --test-faults --image <same digest>` deployed the build of `9aeccf3` (site
  wasm sha256 `0da70974…6c`) in two passes, leaving site a on `a75c1f2d`, b on `a91669ab`, and c on `e3d0105d`.
  `deploy-edge.mjs` with the same `FORMATS_ENABLED=32.1,32.4,32.5` and `LOGINS` left the edge on `d448f2d6` (edge
  wasm sha256 `0a445ae1…ac`). The shard containers of all 11 test repositories (196629-196639) were torn down on
  shards 0, 1, and 2 with `/_bed/kill`, and all 33 answered `{"torn_down":true}`.
