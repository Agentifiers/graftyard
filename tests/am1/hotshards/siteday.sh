#!/bin/bash
# hot-shards' site day on graft-m3c from the deploy worktree ../hs-deploy (origin/main merged with hot-shards-2 and
# the branches queued ahead of it), test-faults, the install's anchor, and the shard image built from hot-shards-2's
# containers/git (tests/am1/hotshards/image.sh). No edge deploy here (edgeday.sh). Running shards are torn down
# afterwards so each starts on the new image (tests/fanout/teardown.py).
#   bash tests/am1/hotshards/siteday.sh <log name>
set -uo pipefail
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0
cd /home/user/workspace/hs-deploy
source /home/user/workspace/repo/tests/am1/lf13/env-m3c.sh
IMG=registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:15f6ed1b1d4b2b08c8adccbce713e89762677c3656dd7ee9a99bdd4bcab025c5
L=/home/user/workspace/repo/tests/am1/raw/hotshards/$1
mkdir -p "$(dirname "$L")"
{ date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5
  node workers/site/deploy.mjs $HOME/.graft-m3c/sites.json --test-faults --anchor $HOME/.graft-m3c/install.toml --image "$IMG" ${SKIP_BUILD:-}
  echo "site exit $?"; date -u +%FT%TZ
  # KEEP_SHARDS=1: the image is unchanged, so running shards keep their disks (a resume across the deploy).
  if [ -z "${KEEP_SHARDS:-}" ]; then python3 tests/fanout/teardown.py --all-nets --pools; echo "teardown exit $?"; fi
  date -u +%FT%TZ; echo DONE; } > "$L" 2>&1
