#!/bin/bash
# A graft-m4a site day of M4-WP2 (days 8 and 10) from a checkout of the deploy branch, release build already made in workers/site/build
# (`worker-build --release`). Same image, --vars, --ops, and GRAFTYARD_HOST as site day 7, plus GRAFTYARD_ACTIVITY=1.
#   bash tests/m4/wp2/hosted/siteday.sh <checkout> <log>
set -uo pipefail
cd "$1"
IMG=registry.cloudflare.com/${CLOUDFLARE_ACCOUNT_ID:?}/graft-m1-site-gitshard-git@sha256:15f6ed1b1d4b2b08c8adccbce713e89762677c3656dd7ee9a99bdd4bcab025c5
{ date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5
  node workers/site/deploy.mjs ~/.graft-m4a/sites.json --anchor ~/.graft-m4a/install.toml --image "$IMG" --skip-build \
    --vars ~/.graft-m4a/site-vars.json --ops graft-m4a-ops \
    --var GRAFTYARD_HOST=graft-m4a-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev --var GRAFTYARD_ACTIVITY=1
  echo "exit $?"; date -u +%FT%TZ; echo DONE; } > "$2" 2>&1
