#!/bin/bash
# One full-load B0-33 row on graft-m2b under costrow.sh's stops, with the host guard (`tests/fanout/hostguard.py`;
# `tests/am1/results/b033-pga.md`, "The host guard"): while a1 sets up, the leader object's CPU per setup commit is
# read from Workers Logs while a1 holds before load (`--hold-file`), and a leader over the line, or one the guard
# could not judge, is re-placed on a fresh repository. At most two re-places; if the third placement is also
# re-placed, the set stops and the sweep runs. pgA's guard (by colo name) let an ORD leader through; this one judges
# what the host measures.
#   TAG=<prefix> [A1_EXTRA="--seed-prs 1000"] [EXPECT=2.5] [RUN_STOP=10] [DAY_STOP=33]
#   EDGE_LOG=tests/am1/raw/runs/fanout-pf-edge.log bash tests/fanout/guardrow.sh   -> /tmp/row-<prefix>.log
# Runs are <prefix>A, <prefix>B, <prefix>C. Everything but the guard is costrow.sh's ROW=b033 row as pfA and pgA ran it.
set -uo pipefail
cd "$(dirname "$0")/../.."
source tests/m3/rows/env.sh
export NODE_USE_ENV_PROXY=1 NO_PROXY="$NO_PROXY,.workers.dev" no_proxy="$NO_PROXY,.workers.dev"
P=${TAG:?} L=/tmp/row-$P.log
echo "start $(date -u +%FT%TZ) $(git rev-parse --short HEAD)" >> $L
python3 tests/fanout/edgever.py "${EDGE_LOG:?}" >> $L 2>&1
need_retire=1
for R in A B C; do
  RF=$(date -u +%FT%TZ)
  if [ $need_retire = 1 ]; then
    RETIRE_LOG=/tmp/reretire-graft-m2b.$P$R.log bash tests/fanout/reretire.sh tests/fanout/reretire/graft-m2b.txt > /tmp/reretire$P$R.out 2>&1; echo "reretire exit $?" >> /tmp/reretire$P$R.out
    if ! grep -q "RERETIRE OK" /tmp/reretire$P$R.out || ! grep -q "^reretire exit 0" /tmp/reretire$P$R.out; then
      echo "$(date -u +%FT%TZ) run $R: RERETIRE FAILED, set stopped" >> $L; break
    fi
  fi
  [ -n "$(git status --porcelain --untracked-files=no)" ] && { echo "$(date -u +%FT%TZ) run $R: tree dirty, set stopped" >> $L; break; }
  DAY=$(PREFIX=graft-m2b python3 tests/fanout/acctcost.py $(date -u +%F)T00:00:00Z 2>/dev/null | head -1 | jq -r '.prefix_usd // 999')
  python3 -c "import sys; sys.exit(0 if float('$DAY') < float('${DAY_STOP:-33}') else 1)" \
    || { echo "$(date -u +%FT%TZ) run $R: graft-m2b at \$$DAY today, set stopped" >> $L; break; }
  echo "$(date -u +%FT%TZ) run $R row starts (graft-m2b \$$DAY today)" >> $L
  # a1 waits before load until the guard writes HOLD (`--hold-file`): `go` starts the load, `stop` (or no file within
  # a1's HOLD_WAIT_MS) ends a1 before load with exit 4, so no row loads on a host the guard did not keep.
  HOLD=/tmp/guard-$P$R.hold; rm -f $HOLD
  env ROW=b033 TAG=$P$R ACCT_FROM=$(date -u +%F)T00:00:00Z STOP=170 RUN_FROM=$RF RUN_STOP=${RUN_STOP:-10} RUN_PREFIX=graft-m2b \
    EXPECT=${EXPECT:-2.5} EDGE_LOG=$EDGE_LOG LEARNERS="enam:2000,weur:1000,apac:1000" A1_EXTRA="${A1_EXTRA:-} --hold-file $HOLD" \
    bash tests/fanout/costrow.sh &
  log=tests/am1/raw/runs/a1-b0-33-f$P$R-b033.log
  # The guard: ask hostguard.py every 20 s until it answers keep (exit 0) or re-place (exit 3). Unknown (exit 2) keeps
  # asking; a1 holds meanwhile. Fails closed: a1's own HOLD_WAIT_MS (300 s) ends it before load with no verdict.
  rid="" verdict="" replaced=0
  while [ -z "$verdict" ]; do
    grep -q "^DONE$" tests/fanout/raw/costrow-$P$R.status 2>/dev/null && break
    sleep 20
    [ -z "$rid" ] && rid=$(awk -v n="f$P$R-b033" '$1==n{print $2}' "$M3_STATE/am1-runs-repos.txt")
    [ -z "$rid" ] && continue
    g=$(python3 -W ignore tests/fanout/hostguard.py graft-m2b-site-a "repo:$rid#1#f0" "$RF" 2>/dev/null); rc=$?
    case $rc in
      0) verdict=$g; echo go > $HOLD; echo "$(date -u +%FT%TZ) run $R repo $rid $g; load released" >> $L ;;
      3) verdict=$g; echo stop > $HOLD; replaced=1
         echo "$(date -u +%FT%TZ) run $R repo $rid $g; GUARD: a1 ended before load, re-placing on a fresh repo" >> $L ;;
      2) ;;
      *) echo "$(date -u +%FT%TZ) run $R guard error (exit $rc), asking again" >> $L ;;
    esac
  done
  # No verdict and the row ended: a1's hold timed out (exit 4) or the row stopped. Counted as a re-place.
  [ -z "$verdict" ] && { replaced=1; echo "$(date -u +%FT%TZ) run $R GUARD: no verdict, a1 ended before load (fails closed)" >> $L; }
  until grep -q "^DONE$" tests/fanout/raw/costrow-$P$R.status 2>/dev/null; do sleep 15; done
  until grep -q "B033DONE" tests/fanout/raw/rows-$P$R.status 2>/dev/null; do sleep 15; [ -n "$(grep -l 'stopped:\|STOP:' tests/fanout/raw/costrow-$P$R.status 2>/dev/null)" ] && break; done
  echo "$(date -u +%FT%TZ) run $R done: $(tail -3 tests/fanout/raw/costrow-$P$R.status | tr '\n' ' ')" >> $L
  grep -q "stopped:\|STOP:" tests/fanout/raw/costrow-$P$R.status && { echo "$(date -u +%FT%TZ) run $R hit a stop, set stopped" >> $L; break; }
  [ $replaced = 0 ] && break
  [ $R = C ] && echo "$(date -u +%FT%TZ) third placement re-placed too: set stopped, swept" >> $L
  need_retire=0
done
echo "ROW${P}DONE" >> $L
