#!/bin/bash
# A site day on graft-m2b from this worktree's HEAD: test-faults, the install's anchor, the wire-fix shard image,
# and --roll-shards through graft-m2b-edge. Audit stays off, as m3-rows ran it.
#   bash tests/m3/wirefix/siteday-m2b.sh <log name> <image>   (output in tests/m3/raw/wirefix/<log name>)
set -uo pipefail
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/home/user/workspace/repo/.worktrees/target-am1
export BED_TOKEN_FILE=$HOME/.graft-m2b/bed.token
cd "$(dirname "$0")/../../.."
{ date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5
  node workers/site/deploy.mjs $HOME/.graft-m2b/sites.json --test-faults --anchor $HOME/.graft-m2b/install.toml \
    --image "$2" --roll-shards https://graft-m2b-edge.<workers-subdomain>.workers.dev
  echo "exit $?"; date -u +%FT%TZ; echo DONE; } > tests/m3/raw/wirefix/$1 2>&1
