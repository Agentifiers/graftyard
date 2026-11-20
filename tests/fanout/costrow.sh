#!/bin/bash
# One hosted row on graft-m2b under a meter stop, as one orb service (LM-1 round 34; round 29's b033-r29.sh made general).
# Before the row: the edge must be the site day's (edgever.py on EDGE_LOG) and the meter plus the row's expected cost
# (EXPECT) must stay under STOP; otherwise nothing runs. During the row the meter is read every 60 s, and at
# STOP - 0.5 every a1.mjs is stopped. After: the row's own end, then a sweep, then the meter.
#   ROW=b034|b033 TAG=<tag> ACCT_FROM=<iso> STOP=120 [RUN_FROM=<iso> RUN_STOP=25] EXPECT=2.5 EDGE_LOG=tests/am1/raw/runs/fanout-mNN-edge.log [IMG=sha256:..]
#   [POOL_PROBE=2:7] bash tests/fanout/costrow.sh   -> tests/fanout/raw/costrow-<tag>.status
set -uo pipefail
cd "$(dirname "$0")/../.."
source ${ROWS_ENV:-tests/m3/rows/env.sh}
source tests/fanout/srvclock.sh
R=tests/fanout/raw ST=tests/fanout/raw/costrow-$TAG.status
[ -e $R/.costrow-$TAG.started ] && exec sleep infinity
touch $R/.costrow-$TAG.started
s() { echo "$(stamp) $*" >> "$ST"; }
# The account meter since ACCT_FROM (operator's 2026-11-16 rule: tests/fanout/acctcost.py is the only meter), and the
# run's own delta since RUN_FROM; STOP is on the account and RUN_STOP on the run.
acct() { python3 tests/fanout/acctcost.py "$1" 2>/dev/null | head -1 | jq -r '.usd // 999' 2>/dev/null || echo 999; }
# With RUN_PREFIX (graft-m2b) the run's delta counts only that install's scripts, for a slot that shares the account.
runusd() { PREFIX=${RUN_PREFIX:-} python3 tests/fanout/acctcost.py "$1" 2>/dev/null | head -1 | jq -r "${RUN_PREFIX:+.prefix_usd // }.usd // 999" 2>/dev/null || echo 999; }
# A failed read is 999, which stops the row; the mt row's seeds were stopped at $0.16 by one read that failed while the
# orb's proxy dropped workers.dev and the API (2026-11-18 18:12Z). A read is retried three times, 20 s apart, before
# 999 stands, so a lasting outage still stops the row and a passing one does not.
meter() {
  local u
  for _ in 1 2 3; do
    u="$(acct "${ACCT_FROM:?}") $(runusd "${RUN_FROM:-$ACCT_FROM}")"
    case "$u" in *999*) sleep 20 ;; *) break ;; esac
  done
  echo "$u"
}
over() { python3 -c 'import sys; a, r = map(float, sys.argv[1].split()); n, s, rs = map(float, sys.argv[2:]); sys.exit(0 if a + n >= s or r + n >= rs else 1)' \
  "$1" "${2:-0}" "$STOP" "${RUN_STOP:-1e9}"; }
python3 tests/fanout/edgever.py "$EDGE_LOG" ${EDGE_SCRIPT:-} >> "$ST" 2>&1 || { s "stopped: the edge is not the site day's"; echo DONE >> "$ST"; exec sleep infinity; }
u=$(meter); s "meter (account, run) $u before $ROW, expected +\$$EXPECT, stop \$$STOP account, \$${RUN_STOP:-none} run"
over "$u" "$EXPECT" && { s "stopped: $u + \$$EXPECT would reach a stop"; echo DONE >> "$ST"; exec sleep infinity; }
case $ROW in
  b034) done_file=$R/b034-$TAG.status; done_mark=DONE
        EDGE_LOG=$EDGE_LOG bash tests/fanout/b034.sh "$TAG" & ;;
  b033) done_file=$R/rows-$TAG.status; done_mark=B033DONE
        STOP_USD=$STOP RUN_STOP_USD=${RUN_STOP:-1e9} bash tests/fanout/b033.sh "$TAG" & ;;
  *) s "unknown ROW $ROW"; echo DONE >> "$ST"; exec sleep infinity ;;
esac
( while ! grep -q "^$done_mark\$\|$done_mark\$" "$done_file" 2>/dev/null; do
    sleep 60; u=$(meter); s "meter (account, run) $u"
    if over "$u" 0.5; then
      # The seeds, the drain and the warm bill too (graft-m1's B0-34 seeds its own networks): every step stops.
      s "STOP: meter $u within \$0.5 of a stop, a1.mjs and the b034 steps stopped"; pkill -f "a1.mjs b0-3[34] "
      pkill -f "tests/fanout/b034(seed|drain|warm)\.mjs"; pkill -f "tests/fanout/b034\.sh"; echo "$done_mark" >> "$done_file"; break; fi
  done ) &
w=$!
until grep -q "$done_mark\$" "$done_file" 2>/dev/null; do sleep 10; done
kill $w 2>/dev/null
s "$ROW ended: $(tail -2 "$done_file" | head -1)"
# HOLD_MONO=1: the row's monorepo network keeps its shards for a 24 h idle measurement (cost thread, 2026-11-20); every
# other shard and pool goes.
keep=""
[ -n "${HOLD_MONO:-}" ] && keep=$(python3 -c "import json,os;print(json.load(open(os.environ['M3_STATE']+'/b034.json'))['mono']['chain'].split(':')[1])")
KEEP_NETS=$keep python3 tests/fanout/teardown.py --sweep >> "$ST" 2>&1; s "sweep exit $? (kept: ${keep:-none})"
u=$(meter); s "meter (account, run) $u after $ROW and the sweep"
echo DONE >> "$ST"
exec sleep infinity
