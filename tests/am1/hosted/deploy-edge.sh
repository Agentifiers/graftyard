#!/bin/bash
# Deploys graft-m1-edge from this worktree: test-faults, the genesis stand-in, the OPS binding, LOGINS={}, and
# FORMATS_ENABLED = graft-m1-edge's list before AM1 plus AM1's 32.8, 32.12, 32.14 and Authz 0x12.3 (18.3).
#   bash tests/am1/hosted/deploy-edge.sh <log name>
set -uo pipefail
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/home/user/workspace/repo/.worktrees/target-am1
cd "$(dirname "$0")/../../.."
FORMATS=32.1,32.4,32.5,32.6,32.7,32.9,16.2,17.2,18.2,20.1,32.10,18.4,32.8,32.12,32.14,18.3
{ date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5
  node tests/m1/integ/deploy-edge.mjs tests/m3/finish/sites4.json graft-m1-edge --test-faults \
    --genesis tests/m1/wp7/genesis.json --ops graft-m1-ops --var FORMATS_ENABLED=$FORMATS --var 'LOGINS={}'
  echo "exit $?"; date -u +%FT%TZ; echo DONE; } > tests/am1/raw/$1 2>&1
grep -E "Current Version ID" tests/am1/raw/$1
