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
