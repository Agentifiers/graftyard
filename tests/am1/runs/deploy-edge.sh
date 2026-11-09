#!/bin/bash
# Deploys graft-m2b-edge from this worktree with graft-m2b's FORMATS_ENABLED and LOGINS
# (tests/m3/wirefix/deploy-edge-m2b.sh). Extra formats, if any, come as $2 (comma-separated) and are appended.
#   bash tests/am1/runs/deploy-edge.sh <log name> [extra formats]   (output in tests/am1/raw/runs/<log name>)
set -uo pipefail
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/home/user/workspace/repo/.worktrees/target-rows
cd "$(dirname "$0")/../../.."
FORMATS=32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2${2:+,$2}
LOGINS='{"7000001":"m2b","7000002":"m2b-writer","7000003":"m2b-reader"}'
{ date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5; echo "FORMATS_ENABLED=$FORMATS"
  node tests/m1/integ/deploy-edge.mjs $HOME/.graft-m2b/sites.json graft-m2b-edge --test-faults \
    --anchor $HOME/.graft-m2b/install.toml --var FORMATS_ENABLED=$FORMATS --var "LOGINS=$LOGINS"
  echo "exit $?"; date -u +%FT%TZ; echo DONE; } > tests/am1/raw/runs/$1 2>&1
grep -E "Current Version ID" tests/am1/raw/runs/$1
