#!/bin/bash
# Site day 5 (L0-16: a held metadata tier is not replayed by the alarm; the edge serves lease commands on the core
# tier while the metadata tier loads), same shard image, no roll; then the edge from the same commit.
cd "$(dirname "$0")/../../.."
IMG=registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:5db19b4ef6b3e9fc4827f30211166e4f1b1a2788460cec6ced708569589c5472
bash tests/am1/hosted/siteday.sh siteday5.log "$IMG" noroll
bash tests/am1/hosted/deploy-edge.sh edge5.log
echo ALLDONE >> tests/am1/raw/edge5.log
