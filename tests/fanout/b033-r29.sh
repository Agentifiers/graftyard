#!/bin/bash
# LM-1 round 29: B0-33 alone on the deployed m37 sites and edge (no site day), under the $155 approval. One orb
# service. Before the row: the edge must be m37's (edgever.py) and the meter, with the row's expected cost (EXPECT,
# default $1.5), must stay under STOP ($155); otherwise nothing runs. During the row: the meter is read every 60 s
# and a1.mjs is stopped once it reaches STOP - 0.5. After: tsave stops, the row's shards are torn down, then a sweep.
#   bash tests/fanout/b033-r29.sh <tag>   -> tests/fanout/raw/r29-<tag>.status and b033.sh's outputs
set -uo pipefail
cd "$(dirname "$0")/../.."
source tests/m3/rows/env.sh
TAG=$1 R=tests/fanout/raw ST=tests/fanout/raw/r29-$1.status
STOP=${STOP:-155} EXPECT=${EXPECT:-1.5}
[ -e $R/.r29-$TAG.started ] && exec sleep infinity
touch $R/.r29-$TAG.started
s() { echo "$(date -u +%FT%TZ) $*" >> "$ST"; }
meter() { T0_FILE=tests/fanout/raw/mem7-t0.t0 bash tests/fanout/cost.sh 2>/dev/null | python3 -c 'import json,sys; print(json.loads(sys.stdin.read()).get("usd", 999))' 2>/dev/null || echo 999; }
python3 tests/fanout/edgever.py tests/am1/raw/runs/fanout-m37-edge.log >> "$ST" 2>&1 || true
live=$(curl -s -H "authorization: Bearer $CLOUDFLARE_API_TOKEN" \
  "https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID/workers/scripts/graft-m2b-edge/deployments" |
  python3 -c "import json,sys; print(json.load(sys.stdin)['result']['deployments'][0]['versions'][0]['version_id'][:8])")
[ "$live" = b4968db8 ] || { s "stopped: graft-m2b-edge serves $live, not m37's b4968db8"; echo DONE >> "$ST"; exec sleep infinity; }
s "edge $live (m37's)"
u=$(meter); s "meter \$$u before the row, expected +\$$EXPECT, stop \$$STOP"
python3 -c "import sys; sys.exit(0 if float('$u') + float('$EXPECT') < float('$STOP') else 1)" ||
  { s "stopped: \$$u + \$$EXPECT would reach \$$STOP"; echo DONE >> "$ST"; exec sleep infinity; }
# Between phases: every 60 s while the row runs; a1.mjs is stopped at STOP - 0.5, and the row ends there.
( while ! grep -q B033DONE $R/rows-$TAG.status 2>/dev/null; do
    sleep 60; u=$(meter); s "meter \$$u"
    if python3 -c "import sys; sys.exit(0 if float('$u') >= float('$STOP') - 0.5 else 1)"; then
      s "STOP: meter \$$u at \$$STOP - 0.5, a1.mjs stopped"; pkill -f "a1.mjs b0-33 --repo f$TAG-b033 "; break; fi
  done ) &
w=$!
STOP_USD=$STOP bash tests/fanout/b033.sh "$TAG" &
until grep -q B033DONE $R/rows-$TAG.status 2>/dev/null; do sleep 10; done
kill $w 2>/dev/null
python3 tests/fanout/teardown.py --sweep >> "$ST" 2>&1; s "sweep exit $?"
u=$(meter); s "meter \$$u after the row and sweep"
echo DONE >> "$ST"
exec sleep infinity
