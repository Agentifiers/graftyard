#!/bin/bash
# The fanout site day on graft-m2b as one orb service: the image (tests/am1/runs/image.sh, tagged am1runs, pushed),
# then the site day (tests/am1/runs/siteday.sh) with that digest, then the edge with the current formats and OPS.
cd "$(dirname "$0")/../.."
[ -e tests/fanout/raw/.siteday.started ] && exec sleep infinity
touch tests/fanout/raw/.siteday.started
bash tests/am1/runs/image.sh fanout-image.log
IMG=$(grep -E "^registry.cloudflare.com/.*@sha256:" tests/am1/raw/runs/fanout-image.log | head -1)
echo "image $IMG" > tests/fanout/raw/siteday.status
[ -n "$IMG" ] || { echo "no image" >> tests/fanout/raw/siteday.status; exec sleep infinity; }
bash tests/am1/runs/siteday.sh fanout-siteday.log "$IMG"
echo "siteday $(grep -E '^exit' tests/am1/raw/runs/fanout-siteday.log)" >> tests/fanout/raw/siteday.status
OPS=graft-m2b-ops bash tests/am1/runs/deploy-edge.sh fanout-edge.log 32.8,32.12,32.14,18.3 >> tests/fanout/raw/siteday.status
echo DONE >> tests/fanout/raw/siteday.status
exec sleep infinity
