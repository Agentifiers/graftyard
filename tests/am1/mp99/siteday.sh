#!/bin/bash
# merge-p99's site day and edge day on graft-m3f, from this checkout, on the image graft-m3f was built with and
# standard-4 shards (install-m3f.md, "Site day" and "Edge day"). Log in tests/am1/raw/mp99-siteday.log.
set -uo pipefail
cd "$(dirname "$0")/../../.."
source tests/am1/lf13/env-m3f.sh
IMG=registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:15f6ed1b1d4b2b08c8adccbce713e89762677c3656dd7ee9a99bdd4bcab025c5
FORMATS=32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2,32.8,32.12,32.14,18.3,32.18
{
  date -u +%FT%TZ; git rev-parse --short HEAD
  node workers/site/deploy.mjs ~/.graft-m3f/sites.json --test-faults --anchor ~/.graft-m3f/install.toml --image "$IMG" \
    --vars ~/.graft-m3f/site-vars.json
  echo "sites exit $?"
  node tests/m1/integ/deploy-edge.mjs ~/.graft-m3f/sites.json graft-m3f-edge --test-faults \
    --anchor ~/.graft-m3f/install.toml --ops graft-m3f-ops --var FORMATS_ENABLED=$FORMATS
  echo "edge exit $?"
  date -u +%FT%TZ; echo DONE
} > tests/am1/raw/mp99-siteday.log 2>&1
