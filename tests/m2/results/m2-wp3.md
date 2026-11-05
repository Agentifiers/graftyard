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
