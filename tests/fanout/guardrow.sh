#!/bin/bash
# One full-load B0-33 row on graft-m2b under costrow.sh's stops, with the host guard (`tests/fanout/hostguard.py`;
# `tests/am1/results/b033-pga.md`, "The host guard"): while a1 sets up, the leader object's CPU per setup commit is
# read from Workers Logs, and a leader over the line is re-placed on a fresh repository before load. At most two
# re-places; if the third placement is also over the line, the set stops before load and the sweep runs. pgA's guard
# (by colo name) let an ORD leader through; this one judges what the host measures.
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
  env ROW=b033 TAG=$P$R ACCT_FROM=$(date -u +%F)T00:00:00Z STOP=170 RUN_FROM=$RF RUN_STOP=${RUN_STOP:-10} RUN_PREFIX=graft-m2b \
    EXPECT=${EXPECT:-2.5} EDGE_LOG=$EDGE_LOG LEARNERS="enam:2000,weur:1000,apac:1000" A1_EXTRA="${A1_EXTRA:-}" \
    bash tests/fanout/costrow.sh &
  log=tests/am1/raw/runs/a1-b0-33-f$P$R-b033.log
  # The guard: until a1 logs its load start, ask hostguard.py every 20 s; stop a1 on `re-place` (exit 3).
  rid="" verdict="" replaced=0
  while ! grep -q "b0-33 run .* leader site" $log 2>/dev/null; do
    sleep 20
    grep -q "^DONE$" tests/fanout/raw/costrow-$P$R.status 2>/dev/null && break
    [ -z "$rid" ] && rid=$(awk -v n="f$P$R-b033" '$1==n{print $2}' "$M3_STATE/am1-runs-repos.txt")
    [ -z "$rid" ] || [ -n "$verdict" ] && continue
    g=$(python3 -W ignore tests/fanout/hostguard.py graft-m2b-site-a "repo:$rid#1#f0" "$RF" 2>/dev/null); rc=$?
    [ $rc = 2 ] && continue
    verdict=$g; echo "$(date -u +%FT%TZ) run $R repo $rid $g (before load)" >> $L
    if [ $rc = 3 ] && ! grep -q "b0-33 run .* leader site" $log 2>/dev/null; then
      echo "$(date -u +%FT%TZ) run $R GUARD: re-placing on a fresh repo; a1 stopped before load" >> $L
      pkill -TERM -f "a1.mjs b0-33 --repo f$P$R-b033 "; replaced=1; break
    fi
  done
  [ $replaced = 0 ] && echo "$(date -u +%FT%TZ) run $R load started, guard: ${verdict:-no verdict by load start}" >> $L
  until grep -q "^DONE$" tests/fanout/raw/costrow-$P$R.status 2>/dev/null; do sleep 15; done
  until grep -q "B033DONE" tests/fanout/raw/rows-$P$R.status 2>/dev/null; do sleep 15; [ -n "$(grep -l 'stopped:\|STOP:' tests/fanout/raw/costrow-$P$R.status 2>/dev/null)" ] && break; done
  echo "$(date -u +%FT%TZ) run $R done: $(tail -3 tests/fanout/raw/costrow-$P$R.status | tr '\n' ' ')" >> $L
  grep -q "stopped:\|STOP:" tests/fanout/raw/costrow-$P$R.status && { echo "$(date -u +%FT%TZ) run $R hit a stop, set stopped" >> $L; break; }
  [ $replaced = 0 ] && break
  [ $R = C ] && echo "$(date -u +%FT%TZ) third placement over the line: set stopped before load, swept" >> $L
  need_retire=0
done
echo "ROW${P}DONE" >> $L
