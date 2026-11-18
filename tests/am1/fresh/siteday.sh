#!/bin/bash
# fresh-repo's site day on graft-m3c from this worktree's HEAD: test-faults, the install's anchor, hot-shards' shard
# image, and SHARD_INSTANCE from <instance> (standard-4 when omitted, as graft-m3c runs since 2026-11-13). Then graft-m3c-edge
# with FORMATS_ENABLED (32.18 included) and `--ops graft-m3c-ops`. Output in tests/am1/raw/fresh/<log name>.
#   bash tests/am1/fresh/siteday.sh <log name> [instance] [edge-only|site-only]
set -uo pipefail
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=$HOME/.cache/fresh/target
export BED_TOKEN_FILE=$HOME/.graft-m3c/bed.token
cd "$(dirname "$0")/../../.."
IMG=registry.cloudflare.com/${CLOUDFLARE_ACCOUNT_ID:?}/graft-m1-site-gitshard-git@sha256:f6d2ef408c766594b541156664de402d3db5107aa480ed3676fe2d62e34fed09
FORMATS=32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2,32.8,32.12,32.14,18.3,32.18
L=tests/am1/raw/fresh/$1
mkdir -p tests/am1/raw/fresh
VARS=$(mktemp)
echo "{\"SHARD_INSTANCE\": \"${2:-standard-4}\"}" > "$VARS"
{ date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5; cat "$VARS"
  if [ "${3:-}" != edge-only ]; then
    node workers/site/deploy.mjs $HOME/.graft-m3c/sites.json --test-faults --anchor $HOME/.graft-m3c/install.toml \
      --image "$IMG" --vars "$VARS"
    echo "site exit $?"; date -u +%FT%TZ
  fi
  [ "${3:-}" = site-only ] || node tests/m1/integ/deploy-edge.mjs $HOME/.graft-m3c/sites.json graft-m3c-edge --test-faults \
    --anchor $HOME/.graft-m3c/install.toml --ops graft-m3c-ops --var FORMATS_ENABLED=$FORMATS
  echo "edge exit $?"; date -u +%FT%TZ; echo DONE; } > "$L" 2>&1
rm -f "$VARS"
