#!/bin/bash
# hot-shards' graft-m3c-edge deploy from the deploy worktree ../hs-deploy: test-faults, the install's anchor,
# `--ops graft-m3c-ops`, and lf4-rest-3's FORMATS_ENABLED (with 32.18). No site day, so members are not reset.
#   bash tests/am1/hotshards/edgeday.sh <log name>
set -uo pipefail
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0
export BED_TOKEN_FILE=$HOME/.graft-m3c/bed.token
cd /home/user/workspace/hs-deploy
source /home/user/workspace/repo/tests/am1/lf13/env-m3c.sh
FORMATS=32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2,32.8,32.12,32.14,18.3,32.18
L=/home/user/workspace/repo/tests/am1/raw/hotshards/$1
# A supervised service restarts a script that exits; a finished deploy must not run again.
grep -q '^DONE' "$L" 2>/dev/null && exec sleep infinity
{ date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5
  node tests/m1/integ/deploy-edge.mjs $HOME/.graft-m3c/sites.json graft-m3c-edge --test-faults \
    --anchor $HOME/.graft-m3c/install.toml --ops graft-m3c-ops --var FORMATS_ENABLED=$FORMATS
  echo "edge exit $?"; date -u +%FT%TZ; echo DONE; } > "$L" 2>&1
