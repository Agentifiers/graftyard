#!/bin/bash
# Redeploys graft-m1-ops (the minting Worker, no public route) from this worktree, bound to the install's
# MintWrapKey secret, so minted tokens carry AM1-WP3's `task` claim. Not a site script.
#   bash tests/am1/hosted/deploy-ops.sh <log name>
set -uo pipefail
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/home/user/workspace/repo/.worktrees/target-am1
cd "$(dirname "$0")/../../.."
{ date -u +%FT%TZ; git rev-parse --short HEAD
  node workers/ops/deploy.mjs graft-m1-ops bfaa0c1573de4775b1b92a12fb89acb4 graft-m1-mint-wrap-key
  echo "exit $?"; date -u +%FT%TZ; echo DONE; } > tests/am1/raw/$1 2>&1
grep -E "Current Version ID" tests/am1/raw/$1
