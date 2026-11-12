#!/bin/bash
# One B0-29-shaped run on an already set-up m3org repository (tests/ld/run.sh's second half), for an orb service:
#   NAME=ld-before2 ID=65544 TAG=before bash tests/ld/load.sh
cd "$(dirname "$0")/../.."
source tests/am1/lf13/env-m3c.sh
ST=tests/ld/raw/run-$TAG.status
[ -e tests/ld/raw/.load-$TAG.started ] && exec sleep infinity
touch tests/ld/raw/.load-$TAG.started
node tests/fanout/k.mjs "repo:$ID" >> "$ST" 2>&1
echo "$(date -u +%FT%TZ) b0-29 start" >> "$ST"
node tests/am1/rest/a1.mjs b0-29 --repo "$NAME" --chain "repo:$ID" --run "$TAG" --seconds "${SECONDS_:-300}" --as-admin \
  > tests/am1/raw/runs/ld-$TAG.log 2>&1
echo "$(date -u +%FT%TZ) b0-29 done $?" >> "$ST"
INSTALL=graft-m3c python3 tests/fanout/teardown.py "$ID" "$ID" > /dev/null 2>&1 && echo "$(date -u +%FT%TZ) torn down $ID" >> "$ST"
echo DONE >> "$ST"
exec sleep infinity
