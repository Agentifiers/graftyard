#!/bin/bash
# One B0-34 hosted row with its own site day on graft-m1 (mt10's run): the site day from the main checked out here
# (tests/am1/b030split/siteday-r4.sh, returning when done), Load34's deploy, a1's selftest, the edge check, setup, then
# costrow.sh's row. Each step that fails writes its line to $GO_STATUS and stops before anything else bills.
#   RUN=b034ma TAG=mt10 SUFFIX=-a1 [ORBCLONE=1] [HOLD_MONO=1] [KEEP_NETS=<held net> HOLD_USD_H=<its $/h>] \
#     amp orb service start b034ma --command 'bash tests/fanout/b034go.sh'
cd "$(dirname "$0")/../.."
source /tmp/proxyenv.sh
source tests/fanout/env-m1.sh
: "${RUN:?}" "${TAG:?}" "${SUFFIX:?}"
GO_STATUS=${GO_STATUS:-/tmp/$RUN-go.status}
st() { echo "$(date -u +%FT%TZ) $*" >> "$GO_STATUS"; }
stop() { st "$*"; exec sleep infinity; }
L=tests/am1/raw/$RUN-siteday.log
SITEDAY_RETURN=1 RUN=$RUN bash tests/am1/b030split/siteday-r4.sh
grep -q '^DONE' "$L" || stop "site day did not finish"
grep -q '^exit 0' "$L" || stop "site day failed"
IMG=$(grep -o '@sha256:[0-9a-f]\{8\}' "$L" | head -1 | tr -d @)
st "site day ok, image $IMG"
bash tests/am1/load34/deploy.sh > "/tmp/$RUN-load34deploy.log" 2>&1 || stop "load34 deploy failed"
node tests/am1/rest/a1.mjs --selftest > "/tmp/$RUN-selftest.log" 2>&1 || stop "a1 selftest failed"
python3 tests/fanout/edgever.py "$L" >> "$GO_STATUS" 2>&1 || stop "the edge is not the site day's"
B034_SUFFIX=$SUFFIX node tests/fanout/b034setup.mjs >> "tests/fanout/raw/b034-$TAG.setup.log" 2>&1 || stop "setup failed"
st "row start"
export ROWS_ENV=tests/fanout/env-m1.sh
HOSTED=${HOSTED:-100} ROW=b034 ACCT_FROM=${ACCT_FROM:-2026-11-19T00:00:00Z} STOP=${STOP:-170} RUN_FROM=$(cat "tests/am1/raw/$RUN.start") \
  RUN_STOP=${RUN_STOP:-8} RUN_PREFIX=graft-m1 EXPECT=${EXPECT:-4} EDGE_LOG=$L IMG=$IMG DRAIN=1 DRAIN_MIN=90 WARM_MIN=45 \
  POOL_PROBE=2:7 B034_DIR=$HOME/b034 exec bash tests/fanout/costrow.sh
