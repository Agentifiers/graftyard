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
