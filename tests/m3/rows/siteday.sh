#!/bin/bash
# A site day on graft-m2b from this worktree's HEAD, test-faults, with --roll-shards. Audit stays off (no GRAFTYARD_AUDIT).
set -euo pipefail
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/home/user/workspace/repo/.worktrees/target-rows
export BED_TOKEN_FILE=$HOME/.graft-m2b/bed.token
cd /home/user/workspace/repo/.worktrees/m3-rows
date -u +%FT%TZ; git rev-parse --short HEAD
node workers/site/deploy.mjs $HOME/.graft-m2b/sites.json --test-faults --anchor $HOME/.graft-m2b/install.toml \
  --image registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:8fa9abd46a0a848bc5d819fdc8eb09b3eaabda4aaf7ec90ae84c57f7e90c0891 \
  --roll-shards https://graft-m2b-edge.<workers-subdomain>.workers.dev
date -u +%FT%TZ; echo SITEDAY-DONE
