#!/bin/bash
# Deploys graft-m2b-ops (workers/ops) bound to the Secrets Store secret graft-m2b-mint-wrap-key, as m3-hosted
# deployed graft-m1-ops. Output in tests/am1/raw/runs/<log name>.
#   bash tests/am1/runs/deploy-ops.sh <log name>
set -uo pipefail
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/home/user/workspace/repo/.worktrees/target-rows
cd "$(dirname "$0")/../../.."
{ date -u +%FT%TZ; git rev-parse --short HEAD
  node workers/ops/deploy.mjs graft-m2b-ops bfaa0c1573de4775b1b92a12fb89acb4 graft-m2b-mint-wrap-key
  echo "exit $?"; date -u +%FT%TZ; echo DONE; } > tests/am1/raw/runs/$1 2>&1
grep -E "Current Version ID" tests/am1/raw/runs/$1
