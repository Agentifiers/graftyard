#!/bin/bash
# Site day 2 (the AM1 hosted code and the git cache image with /validate detail, shards rolled), then the edge and
# graft-ops from the same commit.
cd "$(dirname "$0")/../../.."
IMG=registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:5db19b4ef6b3e9fc4827f30211166e4f1b1a2788460cec6ced708569589c5472
bash tests/am1/hosted/siteday.sh siteday2.log "$IMG"
bash tests/am1/hosted/deploy-edge.sh edge2.log
bash tests/am1/hosted/deploy-ops.sh ops2.log
echo ALLDONE >> tests/am1/raw/ops2.log
