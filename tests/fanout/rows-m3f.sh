#!/bin/bash
# LM-1's three rows split onto graft-m3f (B0-31, B0-32, L0-12; tests/am1/results/lm1-rows-m3f.md), as one orb
# service: rows.sh with the install's environment, learners, k 4, the per-second leader and admission traces, each
# row's shards torn down after it and every shard on graft-m3f swept at the end (teardown.py --sweep, INSTALL from
# env-m3f.sh, so no other install's shards are touched). A second sweep runs after rows.sh as a check, and the
# status ends "SWEEP2 OK" or "SWEEP2 FAILED".
#   TAG=<tag> [ROWS="b0-31 b0-32 l0-12"] [STOP_USD=30] bash tests/fanout/rows-m3f.sh
#   -> tests/fanout/raw/rows-<tag>.status
cd "$(dirname "$0")/../.."
export ROWS_ENV=tests/am1/lf13/env-m3f.sh
source $ROWS_ENV
[ "$INSTALL" = graft-m3f ] || { echo "INSTALL is $INSTALL, not graft-m3f" >&2; exit 1; }
export ROWS=${ROWS:-"b0-31 b0-32 l0-12"} LEARNERS=${LEARNERS:-enam:4000,weur:2000,apac:2000} NO_LF13=1 TEARDOWN=1 TRACE=1
export STOP_USD=${STOP_USD:-30} T0_FILE=${T0_FILE:-tests/am1/raw/m3f-t0.t0}
ST=tests/fanout/raw/rows-$TAG.status
[ -e tests/fanout/raw/.rows-$TAG.started ] && exec sleep infinity
# rows.sh ends with `exec sleep infinity`; run it in the background and wait for ALLDONE.
bash tests/fanout/rows.sh "$TAG" &
until grep -q '^ALLDONE' "$ST" 2>/dev/null; do sleep 20; done
if python3 tests/fanout/teardown.py --sweep >> "$ST" 2>&1; then echo "$(date -u +%FT%TZ) SWEEP2 OK" >> "$ST"
else echo "$(date -u +%FT%TZ) SWEEP2 FAILED" >> "$ST"; fi
echo "$(date -u +%FT%TZ) cost $(bash tests/fanout/cost.sh 2>/dev/null)" >> "$ST"
echo FINISHED >> "$ST"
exec sleep infinity
