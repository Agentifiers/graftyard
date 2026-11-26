#!/usr/bin/env bash
# A site day on graft-m4a (tests/am1/results/install-m4a.md): all three sites from DEPLOY_TREE, already built
# (`worker-build --release` in workers/site), with day 39's flags and the git shard image IMG (default day 42's,
# `5c921511`). DAY names the log, tests/m4/raw/wp4/siteday$DAY.log (default 42). APP_HOOKS=1 adds day 48's
# `--var GRAFTYARD_APP_HOOKS=1` (R24's App deliveries, once members read 32.24). PR_TRIGGER=1 adds
# `--var GRAFTYARD_PR_TRIGGER=1` (B0-35's `pull_request` runs), only on a day after one that shipped format 0x15.4's
# reader to every member (`decisions/DECISIONS.md`, "`pr-trigger`").
set -euo pipefail
tree=${DEPLOY_TREE:-/tmp/m4a-deploy}
log=$(cd "$(dirname "$0")/../raw/wp4" && pwd)/siteday${DAY:-42}.log
IMG=${IMG:-registry.cloudflare.com/${CLOUDFLARE_ACCOUNT_ID:?}/graft-m1-site-gitshard-git@sha256:5c921511196cddc5103eeec9dea2a7e13584e32c555465946a89833fdc7d270c}
cd "$tree"
{ date -u +%FT%TZ; git rev-parse --short HEAD
  node workers/site/deploy.mjs ~/.graft-m4a/sites.json --anchor ~/.graft-m4a/install.toml --image "$IMG" \
    --vars ~/.graft-m4a/site-vars.json --skip-build --ops graft-m4a-ops \
    --var GRAFTYARD_HOST=graft-m4a-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev --var GRAFTYARD_ACTIVITY=1 --var GRAFTYARD_AUDIT=1 ${APP_HOOKS:+--var GRAFTYARD_APP_HOOKS=1} ${PR_TRIGGER:+--var GRAFTYARD_PR_TRIGGER=1} \
    --audit-stream 2a0357004afd44d9a74cf410dd2c715a --run-key ${CF_SECRETS_STORE_ID:?}/graft-m4a-run-service-key
  echo "exit $?"; date -u +%FT%TZ; } 2>&1 | sed "s/${CF_SECRETS_STORE_ID:?}/<store>/g" > "$log"
