#!/bin/bash
# rehydrate-batch's site day and edge day on graft-m3f from the deploy worktree ../rb-deploy (rehydrate-batch merged with
# origin/main or origin/merge-p99), test-faults, the install's anchor, `--vars` (SHARD_INSTANCE=standard-4), and the shard image built
# from rehydrate-batch's containers/git (tests/am1/hotshards/image.sh, TAG=rb). Builds are taken as already made
# (SKIP_BUILD=--skip-build for the site). Shards keep running on the old image until they next start; the cold
# rehydrate tears its own repository's shards down. Log in tests/am1/raw/rehydrate/<log name>.
#   bash tests/am1/rehydrate/siteday.sh <log name>          # SITE_ONLY=1 to leave the edge alone
# Run it as a plain background shell, not an `amp orb service`: a service is restarted when it exits, which deployed
# the sites twice on the first day.
set -uo pipefail
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/home/user/workspace/rb-target
cd /home/user/workspace/rb-deploy
source /home/user/workspace/repo/tests/am1/lf13/env-m3f.sh
IMG=registry.cloudflare.com/${CLOUDFLARE_ACCOUNT_ID:?}/graft-m1-site-gitshard-git@sha256:696907a2838fafd91cb9cab7b9be4a673af54f56b1c811014a4eb0a4e055aec5
FORMATS=32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2,32.8,32.12,32.14,18.3,32.18
L=/home/user/workspace/repo/tests/am1/raw/rehydrate/$1
{ date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5
  node workers/site/deploy.mjs $HOME/.graft-m3f/sites.json --test-faults --anchor $HOME/.graft-m3f/install.toml \
    --image "$IMG" --vars $HOME/.graft-m3f/site-vars.json ${SKIP_BUILD:-}
  echo "site exit $?"; date -u +%FT%TZ
  # SITE_ONLY=1: a site-only redeploy of the same build and image (members reset, shard containers keep their disks).
  [ -n "${SITE_ONLY:-}" ] || node tests/m1/integ/deploy-edge.mjs $HOME/.graft-m3f/sites.json graft-m3f-edge --test-faults \
    --anchor $HOME/.graft-m3f/install.toml --ops graft-m3f-ops --var FORMATS_ENABLED=$FORMATS
  echo "edge exit $?"; date -u +%FT%TZ; echo DONE; } > "$L" 2>&1
