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
IMG=registry.cloudflare.com/${CLOUDFLARE_ACCOUNT_ID:?}/graft-m1-site-gitshard-git@sha256:f6d2ef408c766594b541156664de402d3db5107aa480ed3676fe2d62e34fed09
IMG=${IMG_OVERRIDE:-$IMG}
L=/home/user/workspace/repo/tests/am1/raw/hotshards/$1
# SHARD_INSTANCE: standard-4 from hot-shards-3 on, journey 9's tier size, which the operator approved for test installs.
VARS=$(mktemp); echo "{\"SHARD_INSTANCE\": \"${SHARD_INSTANCE:-standard-4}\"}" > "$VARS"
mkdir -p "$(dirname "$L")"
{ date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5; cat "$VARS"
  node workers/site/deploy.mjs $HOME/.graft-m3c/sites.json --test-faults --anchor $HOME/.graft-m3c/install.toml --image "$IMG" --vars "$VARS" ${SKIP_BUILD:-}
  echo "site exit $?"; date -u +%FT%TZ
  # KEEP_SHARDS=1: the image is unchanged, so running shards keep their disks (a resume across the deploy).
  if [ -z "${KEEP_SHARDS:-}" ]; then python3 tests/fanout/teardown.py --all-nets --pools; echo "teardown exit $?"; fi
  date -u +%FT%TZ; echo DONE; } > "$L" 2>&1
rm -f "$VARS"
