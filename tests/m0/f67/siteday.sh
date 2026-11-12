#!/bin/bash
# f67-liveness's site day on graft-m3c from this worktree's HEAD: test-faults, the install's anchor, hot-shards' shard
# image, then graft-m3c-edge with --ops graft-m3c-ops and FORMATS_ENABLED including 32.18 (narrow-reads' list).
# Log in tests/m0/f67/raw/$1.
#   bash tests/m0/f67/siteday.sh siteday.log
set -uo pipefail
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0
export BED_TOKEN_FILE=$HOME/.graft-m3c/bed.token
cd "$(dirname "$0")/../../.."
mkdir -p tests/m0/f67/raw
L=tests/m0/f67/raw/$1
IMG=registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:7e0c47728cabe0020cd9b3ad3b2fb229f135ba017577aa977e7ec6a3be8905cd
FORMATS=32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2,32.8,32.12,32.14,18.3,32.18
{ date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | grep -v '^??' | head -5
  node workers/site/deploy.mjs $HOME/.graft-m3c/sites.json --test-faults --anchor $HOME/.graft-m3c/install.toml --image "$IMG"
  echo "site exit $?"; date -u +%FT%TZ
  node tests/m1/integ/deploy-edge.mjs $HOME/.graft-m3c/sites.json graft-m3c-edge --test-faults \
    --anchor $HOME/.graft-m3c/install.toml --ops graft-m3c-ops --var FORMATS_ENABLED=$FORMATS
  echo "edge exit $?"; date -u +%FT%TZ; echo DONE; } > "$L" 2>&1
