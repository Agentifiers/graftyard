#!/bin/bash
# Site day 7 (am1-fix): the follower's stall marker (raft), the feed's shared owner read, `/log` extend; same shard
# image, no roll; then the edge (unchanged code, redeployed from this worktree so it matches the sites).
cd "$(dirname "$0")/../../.."
IMG=registry.cloudflare.com/${CLOUDFLARE_ACCOUNT_ID:?}/graft-m1-site-gitshard-git@sha256:5db19b4ef6b3e9fc4827f30211166e4f1b1a2788460cec6ced708569589c5472
bash tests/am1/hosted/siteday.sh siteday7.log "$IMG" noroll
bash tests/am1/hosted/deploy-edge.sh edge7.log
echo ALLDONE >> tests/am1/raw/edge7.log
