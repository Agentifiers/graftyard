#!/bin/bash
# B0-33 as written on graft-m2b with round 17's instruments, for an orb service: rows.sh's b0-33 row (fresh repo,
# eight learners, k 4, 300 s, 5,000 agents), beside memguard.mjs (every voter's wasmMemMb and memSplit every 2 s,
# SIGTERMs a1.mjs past 120 MiB, and keeps BF-14's arrival rows in memory), terms.mjs (leader timeline) and tsave.mjs (each voter's trace each minute).
#   bash tests/fanout/b033.sh <tag>   -> tests/fanout/raw/{rows,memguard,terms,tsave}-<tag>.*
# LEARNERS= (set and empty) places no learners; unset keeps the eight. A1_EXTRA=--lean is the leader-sync A/B's arm B.
set -uo pipefail
cd "$(dirname "$0")/../.."
source tests/m3/rows/env.sh
TAG=$1 R=tests/fanout/raw
repo="f$TAG-b033"
node tests/fanout/memguard.mjs "$repo" 1200 2 120 mem > $R/memguard-$TAG.jsonl 2> $R/memguard-$TAG.err &
node tests/fanout/terms.mjs "$repo" 1200 > $R/terms-$TAG.jsonl 2>&1 &
( while ! rid=$(awk -v n="$repo" '$1==n{print $2}' "$M3_STATE/am1-runs-repos.txt") || [ -z "$rid" ]; do sleep 2; done
  LEARNERS=1 ISOLATES=1 EVERY=10000 DONE_FILE=$R/.tsave-$TAG.done node tests/nr3/tsave.mjs "repo:$rid" > $R/tsave-$TAG.jsonl 2>&1 ) &
T0_FILE=${T0_FILE:-tests/fanout/raw/mem7-t0.t0} ROWS=b0-33 K=4 TEARDOWN=1 NO_LF13=1 STOP_USD=${STOP_USD:-137} LEARNERS=${LEARNERS-enam:4000,weur:2000,apac:2000} \
  bash tests/fanout/rows.sh "$TAG" &
until grep -q ALLDONE $R/rows-$TAG.status 2>/dev/null; do sleep 5; done
touch $R/.tsave-$TAG.done
sleep 70
# The row's `suspect` reports by edge colo, in its status (tests/fanout/README.md, "Client colos"): SUSPECT withholds
# only on two colos, so a row's leader-change count means something only beside these.
{ echo "colos (voters' Reported rows, whole trace):"; python3 tests/fanout/colos.py $R/tsave-$TAG.jsonl 2000-01-01T00:00:00Z 2100-01-01T00:00:00Z; } >> $R/rows-$TAG.status 2>&1
# The leader's setup-phase confirm beside its load confirm (tests/fanout/README.md, "Placement baseline"); with
# LS_EVERY (a1.mjs --ls-refs-shard-every, through A1_EXTRA), the paired minutes and the isolates too.
{ echo "placement baseline:"; python3 tests/fanout/lsrefsab.py $R/tsave-$TAG.jsonl ${LS_EVERY:-}; } >> $R/rows-$TAG.status 2>&1
# Members of the group that shared an isolate (isolates.py): read before trusting a site's tail (feed-per-member).
{ echo "shared isolates:"; python3 tests/fanout/isolates.py $R/tsave-$TAG.jsonl; } >> $R/rows-$TAG.status 2>&1
# Harness outages: the edge's arrivals per second and every line with the outage windows removed (outages.sh).
bash tests/fanout/outages.sh "$TAG" b0-33 "f$TAG-b033"
# why-logs: the edge's 5xx by reason over the row (fivexxwhy.py), and where each refused agent push was refused
# (pushfull.py on a1's per-push log), from the row's first to its last a1 row.
run=$(grep -o 'a1-pushes-b0-33-[a-z0-9]*' $R/rows-$TAG.status 2>/dev/null | tail -1)
[ -z "$run" ] && run=$(ls -t tests/am1/raw/a1-pushes-b0-33-*.jsonl 2>/dev/null | head -1 | xargs -r basename | sed 's/\.jsonl$//')
# A row whose a1 the host guard ended before load (exit 4) wrote no push log: the newest one is another row's, and
# reading it put erBA's 5xx, push refusals and edgefull lines in row B2's erCA and erCB status (2026-11-23).
grep -q '^exit 4$' tests/am1/raw/runs/a1-b0-33-f$TAG-b033.log 2>/dev/null && { run=""; echo "no load (a1 exit 4): no push log to read" >> $R/rows-$TAG.status; }
if [ -n "$run" ]; then
  py=tests/am1/raw/$run.jsonl
  from=$(head -1 $py | jq -r '.start/1000|todate'); to=$(tail -1 $py | jq -r '.end/1000|todate')
  { echo "5xx by reason ($from to $to):"; python3 tests/fanout/fivexxwhy.py "$from" "$to"; } >> $R/rows-$TAG.status 2>&1
  { echo "push refusals by stage:"; python3 tests/fanout/pushfull.py $py; } >> $R/rows-$TAG.status 2>&1
  # edge-refusal (B0-33 row B): the edge's `edgefull` lines and their join to the leader's cuts, flips 2 to 4
  # (`decisions/DECISIONS.md`, "`edge-refusal`"). An edge without them logs none, and the summary says so.
  python3 tests/fanout/edgefull.py fetch "$from" "$to" $R/edgefull-$TAG.jsonl >> $R/rows-$TAG.status 2>&1
  { echo "edge refusals (edgefull):"; python3 tests/fanout/edgefull.py read $R/edgefull-$TAG.jsonl "$TAG"; } >> $R/rows-$TAG.status 2>&1
fi
# The confound on merged main (B0-33 row B): reask-read's top-ups on the leader (`Trace::TopUp`, kind 34), whole trace.
python3 -c "import json,lzma,sys
rows=set()
for l in lzma.open(sys.argv[1],'rt') if sys.argv[1].endswith('.xz') else open(sys.argv[1]):
    d=json.loads(l)
    if 'rows' in d and str(d.get('site'))==str(d.get('leader')): rows.update(tuple(r) for r in d['rows'] if r[1]==34)
print('reask-read top-ups (Trace::TopUp rows on the leader):', len(rows))" $R/tsave-$TAG.jsonl >> $R/rows-$TAG.status 2>&1
echo B033DONE >> $R/rows-$TAG.status
exec sleep infinity
