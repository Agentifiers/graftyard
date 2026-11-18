#!/bin/bash
# Site day 6: main's read gate (1c47d28) merged into am1-hosted, same shard image, no roll; then the edge.
cd "$(dirname "$0")/../../.."
IMG=registry.cloudflare.com/${CLOUDFLARE_ACCOUNT_ID:?}/graft-m1-site-gitshard-git@sha256:5db19b4ef6b3e9fc4827f30211166e4f1b1a2788460cec6ced708569589c5472
bash tests/am1/hosted/siteday.sh siteday6.log "$IMG" noroll
bash tests/am1/hosted/deploy-edge.sh edge6.log
echo ALLDONE >> tests/am1/raw/edge6.log
