#!/bin/bash
# merge-budget-2's site day on graft-m3c from this worktree's HEAD: test-faults, the install's anchor, the shard image
# with `cpuMs` (containers/git changed), and no SHARD_INSTANCE, so shards stay `standard-2` as on every test install.
# Then graft-m3c-edge with lf4-rest-3's FORMATS_ENABLED (32.18 added) and `--ops graft-m3c-ops`. Output in tests/am1/raw/mb2/<log name>.
#   bash tests/am1/mb2/siteday.sh <log name> [edge-only]
set -uo pipefail
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=$HOME/.cache/mb2/target
export BED_TOKEN_FILE=$HOME/.graft-m3c/bed.token
cd "$(dirname "$0")/../../.."
IMG=registry.cloudflare.com/${CLOUDFLARE_ACCOUNT_ID:?}/graft-m1-site-gitshard-git@sha256:5f4f7f937a83a6e95b8d1f292d814a95af958a71705e181deb2ad75f8461a616
FORMATS=32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2,32.8,32.12,32.14,18.3,32.18
L=tests/am1/raw/mb2/$1
mkdir -p tests/am1/raw/mb2
{ date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5
  if [ "${2:-}" != edge-only ]; then
    node workers/site/deploy.mjs $HOME/.graft-m3c/sites.json --test-faults --anchor $HOME/.graft-m3c/install.toml --image "$IMG"
    echo "site exit $?"; date -u +%FT%TZ
  fi
  node tests/m1/integ/deploy-edge.mjs $HOME/.graft-m3c/sites.json graft-m3c-edge --test-faults \
    --anchor $HOME/.graft-m3c/install.toml --ops graft-m3c-ops --var FORMATS_ENABLED=$FORMATS
  echo "edge exit $?"; date -u +%FT%TZ; echo DONE; } > "$L" 2>&1
