#!/bin/bash
# Deploys graft-m2b-ops (workers/ops) bound to the Secrets Store secret graft-m2b-mint-wrap-key, as m3-hosted
# deployed graft-m1-ops. Output in tests/am1/raw/runs/<log name>.
#   bash tests/am1/runs/deploy-ops.sh <log name>
set -uo pipefail
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/home/user/workspace/repo/.worktrees/target-rows
cd "$(dirname "$0")/../../.."
{ date -u +%FT%TZ; git rev-parse --short HEAD
  node workers/ops/deploy.mjs graft-m2b-ops ${CF_SECRETS_STORE_ID:?} graft-m2b-mint-wrap-key
  echo "exit $?"; date -u +%FT%TZ; echo DONE; } 2>&1 | sed "s/${CF_SECRETS_STORE_ID:?}/<store>/g" > tests/am1/raw/runs/$1
grep -E "Current Version ID" tests/am1/raw/runs/$1
