#!/bin/bash
# narrow-reads-3's site day on graft-m3e from this worktree's HEAD (install-m3e.md, "Site day" and "Edge day"):
# test-faults, the install's anchor, hot-shards-2's image, SHARD_INSTANCE from site-vars.json, then graft-m3e-edge with
# --ops graft-m3e-ops and FORMATS_ENABLED through 32.18. Log in tests/nr3/raw/$1.
#   bash tests/nr3/siteday.sh siteday.log
set -uo pipefail
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0
export BED_TOKEN_FILE=$HOME/.graft-m3e/bed.token
cd "$(dirname "$0")/../.."
mkdir -p tests/nr3/raw
L=tests/nr3/raw/$1
IMG=registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:15f6ed1b1d4b2b08c8adccbce713e89762677c3656dd7ee9a99bdd4bcab025c5
FORMATS=32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2,32.8,32.12,32.14,18.3,32.18
{ date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | grep -v '^??' | head -5
  node workers/site/deploy.mjs $HOME/.graft-m3e/sites.json --test-faults --anchor $HOME/.graft-m3e/install.toml --image "$IMG" \
    --vars $HOME/.graft-m3e/site-vars.json
  echo "site exit $?"; date -u +%FT%TZ
  node tests/m1/integ/deploy-edge.mjs $HOME/.graft-m3e/sites.json graft-m3e-edge --test-faults \
    --anchor $HOME/.graft-m3e/install.toml --genesis-signed $HOME/.graft-m3e/genesis.signed --ops graft-m3e-ops \
    --var FORMATS_ENABLED=$FORMATS
  echo "edge exit $?"; date -u +%FT%TZ; echo DONE; } > "$L" 2>&1
