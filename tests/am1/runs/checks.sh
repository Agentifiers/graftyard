#!/bin/bash
# The full checks on this worktree, into tests/am1/raw/runs/checks.log: fmt, workspace clippy, wasm32 clippy for the
# site and the edge (each with and without test-faults) and ops, the workspace tests, the three design checks, and
# the shard's Go tests.
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/home/user/workspace/repo/.worktrees/target-rows
cd "$(dirname "$0")/../../.."
{
r() { echo "== $*"; "$@"; echo "RC=$? :: $*"; }
r cargo fmt --all --check
r cargo clippy --workspace --all-targets -- -D warnings
for c in workers/site workers/edge; do
  (cd $c && r cargo clippy --target wasm32-unknown-unknown -- -D warnings)
  (cd $c && r cargo clippy --target wasm32-unknown-unknown --features test-faults -- -D warnings)
done
(cd workers/ops && r cargo clippy --target wasm32-unknown-unknown -- -D warnings)
r cargo test --workspace
r python3 scripts/check_design.py
r python3 scripts/check_names.py
r python3 scripts/check_links.py
(cd containers/git/server && r go test ./...)
echo CDONE
} > tests/am1/raw/runs/checks.log 2>&1
