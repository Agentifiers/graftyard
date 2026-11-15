#!/usr/bin/env bash
# BF-55's site kill on graft-m4a, through the production deploy path only (a site day of one site), as BF-58's:
#   bf55-kill.sh down <site>  that site's version gets GRAFTYARD_SITE_DOWN=1: every class it hosts answers 503 and runs
#                             no alarm, as if the site were lost (workers/site/src/lib.rs, `site_down`). The driver
#                             (tests/m4/wp3/bf55.mjs) passes the registration shard's leader site.
#   bf55-kill.sh up <site>    that site back on the same build.
# Both deploy the already-built script (`--skip-build`) from DEPLOY_TREE with the install's flags, and write the time
# they finished to /tmp/bf55.<down|up>.
set -euo pipefail
what=${1:?down or up}
site=${2:?site id}
tree=${DEPLOY_TREE:-/tmp/m4a-deploy}
IMG=registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:5c921511196cddc5103eeec9dea2a7e13584e32c555465946a89833fdc7d270c
args=(~/.graft-m4a/sites.json --anchor ~/.graft-m4a/install.toml --image "$IMG" --vars ~/.graft-m4a/site-vars.json
  --skip-build --ops graft-m4a-ops --var GRAFTYARD_HOST=graft-m4a-edge.<workers-subdomain>.workers.dev
  --var GRAFTYARD_ACTIVITY=1 --var GRAFTYARD_AUDIT=1 --audit-stream 2a0357004afd44d9a74cf410dd2c715a
  --run-key bfaa0c1573de4775b1b92a12fb89acb4/graft-m4a-run-service-key --only "$site")
cd "$tree"
case "$what" in
  down) node workers/site/deploy.mjs "${args[@]}" --down "$site" > "/tmp/bf55-down.log" 2>&1 ;;
  up) node workers/site/deploy.mjs "${args[@]}" > "/tmp/bf55-up.log" 2>&1 ;;
  *) echo "down or up" >&2; exit 2 ;;
esac
date +%s%3N > "/tmp/bf55.$what"
