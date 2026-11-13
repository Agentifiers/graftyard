#!/bin/bash
# The fanout site day on graft-m2b as one orb service: the image (tests/am1/runs/image.sh, tagged am1runs, pushed),
# then the site day (tests/am1/runs/siteday.sh) with that digest, then the edge with the current formats and OPS.
# NOROLL=1 skips --roll-shards (shards are derived caches; fresh repositories start on the image).
# TAG names a later day's status and logs (tests/fanout/raw/siteday$TAG.status, fanout$TAG-*.log).
cd "$(dirname "$0")/../.."
T=${TAG:-}
[ -e tests/fanout/raw/.siteday$T.started ] && exec sleep infinity
touch tests/fanout/raw/.siteday$T.started
# IMG=<digest-pinned image> reuses a pushed image (containers/ unchanged) instead of building one.
if [ -z "${IMG:-}" ]; then
  bash tests/am1/runs/image.sh fanout$T-image.log
  IMG=$(grep -E "^registry.cloudflare.com/.*@sha256:" tests/am1/raw/runs/fanout$T-image.log | head -1)
fi
echo "image $IMG" > tests/fanout/raw/siteday$T.status
[ -n "$IMG" ] || { echo "no image" >> tests/fanout/raw/siteday$T.status; exec sleep infinity; }
bash tests/am1/runs/siteday.sh fanout$T-siteday.log "$IMG"
echo "siteday $(grep -E '^exit' tests/am1/raw/runs/fanout$T-siteday.log)" >> tests/fanout/raw/siteday$T.status
OPS=graft-m2b-ops bash tests/am1/runs/deploy-edge.sh fanout$T-edge.log 32.8,32.12,32.14,32.18,18.3 >> tests/fanout/raw/siteday$T.status
echo DONE >> tests/fanout/raw/siteday$T.status
exec sleep infinity
