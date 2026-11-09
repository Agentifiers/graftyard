#!/bin/bash
# Deploys graft-m2b-edge from this worktree with the FORMATS_ENABLED and LOGINS tests/m3/rows/deploy-edge.sh uses.
#   bash tests/m3/wirefix/deploy-edge-m2b.sh <log name>   (output in tests/m3/raw/wirefix/<log name>)
set -uo pipefail
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/home/user/workspace/repo/.worktrees/target-am1
cd "$(dirname "$0")/../../.."
FORMATS=32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2
LOGINS='{"7000001":"m2b","7000002":"m2b-writer","7000003":"m2b-reader"}'
{ date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5
  node tests/m1/integ/deploy-edge.mjs $HOME/.graft-m2b/sites.json graft-m2b-edge --test-faults \
    --anchor $HOME/.graft-m2b/install.toml --var FORMATS_ENABLED=$FORMATS --var "LOGINS=$LOGINS"
  echo "exit $?"; date -u +%FT%TZ; echo DONE; } > tests/m3/raw/wirefix/$1 2>&1
grep -E "Current Version ID" tests/m3/raw/wirefix/$1
