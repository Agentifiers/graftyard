#!/bin/bash
# Site day 4 (the feed lag fix, same shard image; tests/am1/hosted/roll.mjs moves the remaining shards), then
# the edge from the same commit.
cd "$(dirname "$0")/../../.."
IMG=registry.cloudflare.com/${CLOUDFLARE_ACCOUNT_ID:?}/graft-m1-site-gitshard-git@sha256:5db19b4ef6b3e9fc4827f30211166e4f1b1a2788460cec6ced708569589c5472
bash tests/am1/hosted/siteday.sh siteday4.log "$IMG" noroll
bash tests/am1/hosted/deploy-edge.sh edge4.log
echo ALLDONE >> tests/am1/raw/edge4.log
