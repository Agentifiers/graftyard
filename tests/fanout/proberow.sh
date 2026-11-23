#!/bin/bash
# The `seal-batch` probe as one short test-faults row on graft-m2b under a meter stop (`decisions/DECISIONS.md`,
# "`seal-batch`"): a fresh repository, `sqlprobe.mjs` on its leader, that member's Workers Logs for the window, the fit,
# then a sweep and the meter. Nothing runs if the meter plus EXPECT reaches STOP; the probe is stopped at STOP - 0.5.
#   TAG=<tag> ACCT_FROM=<iso> STOP=<usd> [EXPECT=0.5] [REPS=3] bash tests/fanout/proberow.sh
#   -> tests/fanout/raw/proberow-<tag>.status, probe-<tag>.jsonl, probe-<tag>-wl.json, probe-<tag>.fit
set -uo pipefail
cd "$(dirname "$0")/../.."
source ${ROWS_ENV:-tests/m3/rows/env.sh}
R=tests/fanout/raw ST=$R/proberow-$TAG.status
s() { echo "$(date -u +%FT%TZ) $*" >> "$ST"; }
meter() { PREFIX=graft-m2b python3 tests/fanout/acctcost.py "$ACCT_FROM" 2>/dev/null | head -1 | jq -r '.prefix_usd // .usd // 999' 2>/dev/null || echo 999; }
over() { python3 -c 'import sys; sys.exit(0 if float(sys.argv[1]) + float(sys.argv[2]) >= float(sys.argv[3]) else 1)' "$1" "$2" "$STOP"; }
u=$(meter); s "meter $u before the probe, expected +\$${EXPECT:-0.5}, stop \$$STOP"
over "$u" "${EXPECT:-0.5}" && { s "stopped: $u + \$${EXPECT:-0.5} would reach the stop"; echo DONE >> "$ST"; exit 0; }
node tests/am1/runs/mkrepos.mjs "probe-$TAG" >> "$ST" 2>&1 || { s "no repository"; echo DONE >> "$ST"; exit 1; }
ID=$(awk -v n="probe-$TAG" '$1 == n { print $2 }' "$M3_STATE/am1-runs-repos.txt")
s "repository $ID"
sleep 20
FROM=$(date -u +%FT%TZ)
node tests/fanout/sqlprobe.mjs "repo:$ID" "" "${REPS:-3}" > $R/probe-$TAG.jsonl 2>> "$ST" &
P=$!
while kill -0 $P 2>/dev/null; do
  sleep 30; u=$(meter); s "meter $u"
  over "$u" 0.5 && { s "STOP: meter $u within \$0.5 of the stop"; kill $P; break; }
done
wait $P; s "probe exit $?"
TO=$(date -u -d '+1 min' +%FT%TZ)
SITE=$(grep "^{" $R/probe-$TAG.jsonl | head -1 | jq -r .site)
SVC=graft-m2b-site-$(echo abc | cut -c$((SITE + 1)))
sleep 90
python3 tests/fanout/wlobj.py fetch "$SVC" "repo:$ID#1#f$SITE" "$FROM" "$TO" $R/probe-$TAG-wl.json >> "$ST" 2>&1
python3 tests/fanout/sqlprobe.py $R/probe-$TAG.jsonl $R/probe-$TAG-wl.json > $R/probe-$TAG.fit 2>&1
s "fit exit $?"
python3 tests/fanout/teardown.py --sweep >> "$ST" 2>&1; s "sweep exit $?"
u=$(meter); s "meter $u after"
echo DONE >> "$ST"
