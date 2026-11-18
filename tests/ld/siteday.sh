#!/bin/bash
# learner-dumps' site day on graft-m3c from this worktree's HEAD: site only (test-faults, the install's anchor, the shard
# image graft-m3c already runs), no edge deploy, so graft-m3c-edge and its OPS binding stay. Log in tests/ld/raw/$1.
#   bash tests/ld/siteday.sh siteday.log
set -uo pipefail
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/home/user/workspace/repo/.worktrees/target-rows
export BED_TOKEN_FILE=$HOME/.graft-m3c/bed.token
cd "$(dirname "$0")/../.."
IMG=registry.cloudflare.com/${CLOUDFLARE_ACCOUNT_ID:?}/graft-m1-site-gitshard-git@sha256:172216ad0cbdf8779b0c20a698a0b5040a3de4295581408698ea3b157a8df3e9
L=tests/ld/raw/$1
{ date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5
  node workers/site/deploy.mjs $HOME/.graft-m3c/sites.json --test-faults --anchor $HOME/.graft-m3c/install.toml --image "$IMG"
  echo "site exit $?"; date -u +%FT%TZ; echo DONE; } > "$L" 2>&1
