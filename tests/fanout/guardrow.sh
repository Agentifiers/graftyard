#!/bin/bash
# One full-load B0-33 row on graft-m2b under costrow.sh's stops, with the host guard (`tests/fanout/hostguard.py`;
# `tests/am1/results/b033-pga.md`, "The host guard"): while a1 sets up, the leader object's CPU per setup commit is
# read from Workers Logs while a1 holds before load (`--hold-file`), and a leader over the line, or one the guard
# could not judge, is re-placed on a fresh repository. At most two re-places; if the third placement is also
# re-placed, the set stops and the sweep runs. pgA's guard (by colo name) let an ORD leader through; this one judges
# what the host measures.
# GUARD_WANT=slow inverts it for a row that measures slow-host behaviour (B0-33 row B, edge refusal: on a kept host
# the leader never advertised a wait past the 5 s bound, `tests/am1/results/edge-refusal.md`). A leader over the line
# is kept and one at or under it is re-placed. Unknown still fails closed, and the line is not widened. Such a row
# reads slow hosts only, against baselines in the same setup-CPU band, never as typical-host behaviour.
# GUARD_WANT=band (edge-refusal-bound's rerun) keeps a leader whose reading falls inside one of GUARD_BAND's bands (ms
# of leader CPU a setup commit, `lo-hi` separated by spaces, default the lead's two: 31.0-31.5, blA and blC2's, and
# 39.7-46.1, pfA, pgA and ncA's) and re-places any other. The log names the band and the baseline the run is judged
# against before load. Unknown still fails closed; if no placement lands in a band the set stops without load (not a
# fail), and the bands are not widened.
#   TAG=<prefix> [GUARD_WANT=fast|slow|band] [GUARD_BAND="31.0-31.5 39.7-46.1"] [A1_EXTRA="--seed-prs 1000"]
#   [EXPECT=2.5] [RUN_STOP=10] [DAY_STOP=33]
#   EDGE_LOG=tests/am1/raw/runs/fanout-pf-edge.log bash tests/fanout/guardrow.sh   -> /tmp/row-<prefix>.log
# Runs are <prefix>A, <prefix>B, <prefix>C. Everything but the guard is costrow.sh's ROW=b033 row as pfA and pgA ran it.
set -uo pipefail
cd "$(dirname "$0")/../.."
source tests/m3/rows/env.sh
export NODE_USE_ENV_PROXY=1 NO_PROXY="$NO_PROXY,.workers.dev" no_proxy="$NO_PROXY,.workers.dev"
P=${TAG:?} L=/tmp/row-$P.log WANT=${GUARD_WANT:-fast}
case $WANT in fast) KEEP=0 MOVE=3 ;; slow) KEEP=3 MOVE=0 ;; band) KEEP=10 MOVE=11 ;;
  *) echo "GUARD_WANT=$WANT: fast, slow or band" >&2; exit 1 ;; esac
BAND=${GUARD_BAND:-31.0-31.5 39.7-46.1}
# band: hostguard's verdict line (`... , 36.1 ms of leader CPU a setup commit ...`) read against BAND. Prints the band and
# its baseline and exits 10 inside one, 11 outside every one; any other hostguard answer passes through.
band_of() {
  local out rc; out=$(python3 -W ignore tests/fanout/hostguard.py "$@" 2>/dev/null); rc=$?
  [ $rc = 0 ] || [ $rc = 3 ] || { echo "$out"; return $rc; }
  python3 - "$out" "$BAND" <<'PY'
import re, sys
out, bands = sys.argv[1], sys.argv[2].split()
m = re.search(r"([0-9.]+) ms of leader CPU a setup commit", out)
per = float(m.group(1)) if m else None
base = {"31.0-31.5": "blA/blC2 (before the drain change)", "39.7-46.1": "pfA/pgA"}
hit = next((b for b in bands if per is not None and float(b.split("-")[0]) <= per <= float(b.split("-")[1])), None)
print(f"{out.strip()}; band {hit}, judged against {base.get(hit, 'no named baseline')}" if hit else
      f"{out.strip()}; in no band of {' '.join(bands)}")
sys.exit(10 if hit else 11)
PY
}
echo "start $(date -u +%FT%TZ) $(git rev-parse --short HEAD) guard wants a $WANT host" >> $L
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
  # The guard: ask hostguard.py every 20 s until it answers keep (exit 0) or re-place (exit 3); with GUARD_WANT=slow
  # the two swap. Unknown (exit 2) keeps asking; a1 holds meanwhile. Fails closed: a1's own HOLD_WAIT_MS (300 s) ends
  # it before load with no verdict.
  rid="" verdict="" replaced=0
  while [ -z "$verdict" ]; do
    grep -q "^DONE$" tests/fanout/raw/costrow-$P$R.status 2>/dev/null && break
    sleep 20
    [ -z "$rid" ] && rid=$(awk -v n="f$P$R-b033" '$1==n{print $2}' "$M3_STATE/am1-runs-repos.txt")
    [ -z "$rid" ] && continue
    if [ $WANT = band ]; then g=$(band_of graft-m2b-site-a "repo:$rid#1#f0" "$RF"); rc=$?
    else g=$(python3 -W ignore tests/fanout/hostguard.py graft-m2b-site-a "repo:$rid#1#f0" "$RF" 2>/dev/null); rc=$?; fi
    case $rc in
      $KEEP) verdict=$g; echo go > $HOLD; echo "$(date -u +%FT%TZ) run $R repo $rid $g; $WANT host wanted, load released" >> $L ;;
      $MOVE) verdict=$g; echo stop > $HOLD; replaced=1
         echo "$(date -u +%FT%TZ) run $R repo $rid $g; GUARD: not a $WANT host, a1 ended before load, re-placing on a fresh repo" >> $L ;;
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
  # a1's `--advert-probe` failing is the build, not the host: another placement would fail it too.
  grep -q "advert probe failed" $log 2>/dev/null && { echo "$(date -u +%FT%TZ) run $R ADVERT PROBE FAILED before load, set stopped" >> $L; break; }
  # Flip 1 (edge-refusal) is an invariant: a1 ended the window on it, and the set stops for the lead.
  grep -q "FLIP1:" $log 2>/dev/null && { echo "$(date -u +%FT%TZ) run $R FLIP1: a non-agent write refused by the advert, set stopped" >> $L; break; }
  [ $replaced = 0 ] && break
  [ $R = C ] && echo "$(date -u +%FT%TZ) third placement re-placed too: set stopped, swept$([ $WANT = band ] && echo '; no placement in a band, no load (not a fail)')" >> $L
  need_retire=0
done
echo "ROW${P}DONE" >> $L
