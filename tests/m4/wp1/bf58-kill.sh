#!/usr/bin/env bash
# BF-58's site kill on graft-m4a, through the production deploy path only (a site day of site a alone):
#   bf58-kill.sh down   site a's version gets GRAFTYARD_SITE_DOWN=1: every class it hosts answers 503 and runs no
#                       alarm, as if the site were lost (workers/site/src/lib.rs, `site_down`)
#   bf58-kill.sh up     site a back, with GRAFTYARD_HOOKSEQ_RESET_BEFORE set to now: every `HookSequencer` store on
#                       site a starts empty, as a store lost with its site
# Both deploy the already-built script (`--skip-build`) from DEPLOY_TREE, with the install's flags. Each writes the
# time it finished to /tmp/bf58.<down|up>, which tests/m4/wp1/bf58.mjs reads as the mark.
set -euo pipefail
what=${1:?down or up}
tree=${DEPLOY_TREE:-/tmp/m4a-deploy}
IMG=registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:099a022c23a31d2ca50ccb745a1b002cbe8af1f4c571fff713bc1a8b698a1440
args=(~/.graft-m4a/sites.json --anchor ~/.graft-m4a/install.toml --image "$IMG" --vars ~/.graft-m4a/site-vars.json
  --skip-build --ops graft-m4a-ops --var GRAFTYARD_HOST=graft-m4a-edge.<workers-subdomain>.workers.dev --var GRAFTYARD_ACTIVITY=1 --only 0)
cd "$tree"
case "$what" in
  down) node workers/site/deploy.mjs "${args[@]}" --down 0 ;;
  up) node workers/site/deploy.mjs "${args[@]}" --var "GRAFTYARD_HOOKSEQ_RESET_BEFORE=$(date +%s%3N)" ;;
  *) echo "down or up" >&2; exit 2 ;;
esac
date +%s%3N > "/tmp/bf58.$what"
