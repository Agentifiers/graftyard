#!/bin/bash
# A site day on graft-m1 (sites a to d) from this worktree's HEAD: test-faults, GRAFTYARD_AUDIT=1 as the sites run it,
# the install's genesis stand-in, the pinned git shard image, and --roll-shards through graft-m1-edge.
#   bash tests/am1/hosted/siteday.sh <log name>      (output in tests/am1/raw/<log name>)
set -uo pipefail
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/home/user/workspace/repo/.worktrees/target-am1
export BED_TOKEN_FILE=$HOME/.graft-wp6/bed.token
cd "$(dirname "$0")/../../.."
mkdir -p tests/am1/raw
echo '{"GRAFTYARD_AUDIT":"1"}' > /tmp/am1-site-vars.json
{ date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5
  node workers/site/deploy.mjs tests/m3/finish/sites4.json --genesis tests/m1/wp7/genesis.json --test-faults \
    --vars /tmp/am1-site-vars.json \
    --image registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:8fa9abd46a0a848bc5d819fdc8eb09b3eaabda4aaf7ec90ae84c57f7e90c0891 \
    --roll-shards https://graft-m1-edge.<workers-subdomain>.workers.dev
  echo "exit $?"; date -u +%FT%TZ; echo DONE; } > tests/am1/raw/$1 2>&1
