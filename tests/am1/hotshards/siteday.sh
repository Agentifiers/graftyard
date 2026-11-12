#!/bin/bash
# hot-shards' site day on graft-m3c from the deploy worktree ../hs-deploy (hot-shards, main, and origin/b030-split's
# archive drain), test-faults, the install's anchor, and the shard image built from hot-shards' containers/git
# (graft-m1-site-gitshard-git:hot-shards). No edge deploy: graft-m3c-edge keeps its OPS binding and FORMATS_ENABLED.
# Running shards are torn down afterwards so each starts on the new image (tests/fanout/teardown.py).
#   bash tests/am1/hotshards/siteday.sh <log name>
set -uo pipefail
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0
cd /home/user/workspace/hs-deploy
source /home/user/workspace/repo/tests/am1/lf13/env-m3c.sh
IMG=registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:60e8a94dcdea1b3c5be114abfcbdfc4a385aaada9b2099db20ca54551a337eab
L=/home/user/workspace/repo/tests/am1/raw/hotshards/$1
mkdir -p "$(dirname "$L")"
{ date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5
  node workers/site/deploy.mjs $HOME/.graft-m3c/sites.json --test-faults --anchor $HOME/.graft-m3c/install.toml --image "$IMG"
  echo "site exit $?"; date -u +%FT%TZ
  python3 tests/fanout/teardown.py --all-nets --pools
  echo "teardown exit $?"; date -u +%FT%TZ; echo DONE; } > "$L" 2>&1
