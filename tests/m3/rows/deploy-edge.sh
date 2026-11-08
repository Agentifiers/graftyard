#!/bin/bash
# Deploys graft-m2b-edge from this worktree: a test-faults build with the install's anchor, the M3 formats as
# graft-m1-edge enables them (FORMATS_ENABLED), and the seed's LOGINS kept. Output goes to tests/m3/raw/rows/<log>.
#   bash tests/m3/rows/deploy-edge.sh <log name>
set -euo pipefail
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/home/user/workspace/repo/.worktrees/target-rows
cd "$(dirname "$0")/../../.."
FORMATS=32.1,32.4,32.5,32.6,32.7,32.9,16.2,17.2,18.2,20.1
LOGINS='{"7000001":"m2b","7000002":"m2b-writer","7000003":"m2b-reader"}'
{ date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5
  node tests/m1/integ/deploy-edge.mjs $HOME/.graft-m2b/sites.json graft-m2b-edge --test-faults \
    --anchor $HOME/.graft-m2b/install.toml --var FORMATS_ENABLED=$FORMATS --var "LOGINS=$LOGINS"
  date -u +%FT%TZ; } > tests/m3/raw/rows/$1 2>&1
grep -E "Current Version ID" tests/m3/raw/rows/$1
