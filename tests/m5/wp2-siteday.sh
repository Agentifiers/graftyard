#!/bin/bash
# M5-WP2's site day on graft-m3c from this worktree's HEAD: test-faults, the install's anchor, fresh-repo's shard image,
# SHARD_INSTANCE standard-4, `--ops graft-m3c-ops`, and GRAFTYARD_STALE_MAX_MS (test-faults only), which lowers
# STALE_MAX in the members' cached views so BF-48 sees the bound end an uncommitted flag within the run. The next site
# day without the variable restores the compiled 24 h. Sites only: graft-m3c-edge is not changed. Output in
# tests/m5/results/raw/<log name>.
#   bash tests/m5/wp2-siteday.sh <log name> [stale max ms]
set -uo pipefail
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0
cd "$(dirname "$0")/../.."
IMG=registry.cloudflare.com/${CLOUDFLARE_ACCOUNT_ID:?}/graft-m1-site-gitshard-git@sha256:f6d2ef408c766594b541156664de402d3db5107aa480ed3676fe2d62e34fed09
L=tests/m5/results/raw/$1
mkdir -p tests/m5/results/raw
VARS=$(mktemp)
echo '{"SHARD_INSTANCE": "standard-4"}' > "$VARS"
{ date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5
  node workers/site/deploy.mjs $HOME/.graft-m3c/sites.json --test-faults --anchor $HOME/.graft-m3c/install.toml \
    --image "$IMG" --vars "$VARS" --ops graft-m3c-ops --var GRAFTYARD_STALE_MAX_MS=${2:-120000}
  echo "site exit $?"; date -u +%FT%TZ; echo DONE; } > "$L" 2>&1
rm -f "$VARS"
