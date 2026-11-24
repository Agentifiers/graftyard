#!/bin/bash
# B0-33 as written on graft-m2b with round 17's instruments, for an orb service: rows.sh's b0-33 row (fresh repo,
# eight learners, k 4, 300 s, 5,000 agents), beside memguard.mjs (every voter's wasmMemMb and memSplit every 2 s,
# SIGTERMs a1.mjs past 120 MiB, and keeps BF-14's arrival rows in memory), terms.mjs (leader timeline) and tsave.mjs (each voter's trace each minute).
#   bash tests/fanout/b033.sh <tag>   -> tests/fanout/raw/{rows,memguard,terms,tsave}-<tag>.*
# LEARNERS= (set and empty) places no learners; unset keeps the eight. A1_EXTRA=--lean is the leader-sync A/B's arm B.
# B033_SECONDS (default 300) is the load window; rows.sh's a1 flags, the readers' window and memguard's and terms.mjs's
# run (setup B033_SETUP_S, default 660 s, clA's 09:55 to 10:06, plus the load plus 240 s) follow it. After the row the
# leader object's Workers Logs over the load window, a minute either side, go to raw/leaderlogs-<tag>.json.xz
# (wlleader.py) in the background, so the sweep does not wait on them. DRY=1 prints the bounds and starts nothing.
set -uo pipefail
cd "$(dirname "$0")/../.."
TAG=$1 R=tests/fanout/raw
repo="f$TAG-b033"
export B033_SECONDS=${B033_SECONDS:-300}
SECS=$B033_SECONDS MON=$(( ${B033_SETUP_S:-660} + B033_SECONDS + 240 ))
if [ -n "${DRY:-}" ]; then
  row=b0-33; eval "$(sed -n '/^  case \$row in$/,/^  esac$/p' tests/fanout/rows.sh)"
  echo "b033.sh $TAG: load ${SECS} s; rows.sh a1 flags: $flags"
  echo "memguard and terms.mjs: $MON s from the row's start (setup ${B033_SETUP_S:-660} + load $SECS + 240)"
  echo "readers' window: a1's announced load start to start + $SECS s"
  echo "leaderlogs: the leader object from 60 s before load to 60 s after it ($((SECS + 120)) s) -> $R/leaderlogs-$TAG.json.xz"
  exit 0
fi
source tests/m3/rows/env.sh
node tests/fanout/memguard.mjs "$repo" $MON 2 120 mem > $R/memguard-$TAG.jsonl 2> $R/memguard-$TAG.err &
node tests/fanout/terms.mjs "$repo" $MON > $R/terms-$TAG.jsonl 2>&1 &
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
# A row whose a1 the host guard ended before load (exit 4) wrote no push log, and taking the newest one put erBA's 5xx,
# push refusals and edgefull lines in row B2's erCA and erCB status (2026-11-23). Any a1 that announced no load window
# wrote none either: g4A's a1 failed in setup (exit 1, a 503 minting the App's
# token) and its status took g3A's 5xx, refusal-shape, pushfull and edgefull sections (2026-11-24). So the readers run
# only when this row's a1 log announces its load window (`b0-33 run <RUN>: ... s from`), and then on that run's log.
a1log=tests/am1/raw/runs/a1-b0-33-f$TAG-b033.log
loadrun=$(grep -E ' b0-33 run [a-z0-9]+: .* s from ' "$a1log" 2>/dev/null | tail -1 | sed -E 's/.* b0-33 run ([a-z0-9]+):.*/\1/')
if [ -z "$loadrun" ]; then
  run=""; echo "no load (a1 $(grep -E '^exit [0-9]+$' "$a1log" 2>/dev/null | tail -1)): no push log to read" >> $R/rows-$TAG.status
else
  run=a1-pushes-b0-33-$loadrun
fi
if [ -n "$run" ]; then
  py=tests/am1/raw/$run.jsonl
  from=$(head -1 $py | jq -r '.start/1000|todate'); to=$(tail -1 $py | jq -r '.end/1000|todate')
  { echo "5xx by reason ($from to $to):"; python3 tests/fanout/fivexxwhy.py "$from" "$to"; } >> $R/rows-$TAG.status 2>&1
  # The refusal-shape line as ruled (FRAME, 2026-11-23): a1's non-2xx answers by class, rule and Retry-After.
  { echo "refusal shape:"; python3 tests/fanout/refusalshape.py "tests/am1/raw/${run/a1-pushes-/a1-}.jsonl" $R/rows-$TAG.status; } >> $R/rows-$TAG.status 2>&1
  # user-demote: the user-token line (the fast user demoted) and the Human guard (no Human writer demoted).
  python3 tests/fanout/demoted.py "tests/am1/raw/${run/a1-pushes-/a1-}.jsonl" >> $R/rows-$TAG.status 2>&1
  { echo "push refusals by stage:"; python3 tests/fanout/pushfull.py $py; } >> $R/rows-$TAG.status 2>&1
  # edge-refusal (B0-33 row B): the edge's `edgefull` lines and their join to the leader's cuts, flips 2 to 4
  # (`decisions/DECISIONS.md`, "`edge-refusal`"). An edge without them logs none, and the summary says so.
  python3 tests/fanout/edgefull.py fetch "$from" "$to" $R/edgefull-$TAG.jsonl >> $R/rows-$TAG.status 2>&1
  { echo "edge refusals (edgefull):"; python3 tests/fanout/edgefull.py read $R/edgefull-$TAG.jsonl "$TAG"; } >> $R/rows-$TAG.status 2>&1
  # G40: each shard container's vCPU over the load window a1 announced (instcpu.py, raw kept), summed per site, so a
  # home site at its cap with the others idle (concentration) reads apart from busy refusals with spare vCPU (a dearer
  # validate). The push log's own window starts with setup's pushes, minutes before load.
  lfrom=$(grep -E " b0-33 run $loadrun: .* s from " "$a1log" | tail -1 | sed -E 's/.* s from ([^ ,]+).*/\1/')
  lto=$(python3 -c "import sys,datetime as d;print((d.datetime.fromisoformat(sys.argv[1].replace('Z','+00:00'))+d.timedelta(seconds=int(sys.argv[2]))).strftime('%Y-%m-%dT%H:%M:%SZ'))" "$lfrom" "$SECS")
  python3 tests/fanout/instcpu.py "$lfrom" "$lto" git# > $R/instcpu-$TAG.jsonl 2>> $R/rows-$TAG.status
  { echo "shard vCPU (instcpu, G40):"; python3 -c "import collections,json,sys
site=collections.Counter()
for l in open(sys.argv[1]):
    d=json.loads(l); v=list(d['vcpu_by_minute'].values()) or [0]; site[(d['name'].split('#')+['?'])[1]]+=sum(v)
    if max(v,default=0)>=0.5: print(f\"  {d['name']}: peak {max(v):.2f}, mean {sum(v)/len(v):.2f} vCPU over {len(v)} min\")
print('  vCPU-minutes by site: '+', '.join(f'{k} {v:.1f}' for k,v in sorted(site.items())))" $R/instcpu-$TAG.jsonl; } >> $R/rows-$TAG.status 2>&1
fi
# The confound on merged main (B0-33 row B): reask-read's top-ups on the leader (`Trace::TopUp`, kind 34), whole trace.
python3 -c "import json,lzma,sys
rows=set()
for l in lzma.open(sys.argv[1],'rt') if sys.argv[1].endswith('.xz') else open(sys.argv[1]):
    d=json.loads(l)
    if 'rows' in d and str(d.get('site'))==str(d.get('leader')): rows.update(tuple(r) for r in d['rows'] if r[1]==34)
print('reask-read top-ups (Trace::TopUp rows on the leader):', len(rows))" $R/tsave-$TAG.jsonl >> $R/rows-$TAG.status 2>&1
# The leader object's Workers Logs (wlleader.py) over the load window, a minute either side: the archive and alarm-step
# lines against memguard's samples. The leader is the one a1 announced at load start; a later leader is not fetched.
if [ -n "${run:-}" ] && [ -n "${lfrom:-}" ]; then
  lsite=$(grep -E " b0-33 run $loadrun: " "$a1log" | tail -1 | sed -nE 's/.*leader site ([0-9]+).*/\1/p')
  rid=$(awk -v n="$repo" '$1==n{print $2}' "$M3_STATE/am1-runs-repos.txt")
  if [ -n "$lsite" ] && [ -n "$rid" ]; then
    span() { python3 -c "import sys,datetime as d;print((d.datetime.fromisoformat(sys.argv[1].replace('Z','+00:00'))+d.timedelta(seconds=int(sys.argv[2]))).strftime('%Y-%m-%dT%H:%M:%SZ'))" "$1" "$2"; }
    la=$(span "$lfrom" -60) lb=$(span "$lfrom" $((SECS + 60)))
    svc=graft-m2b-site-$(echo abc | cut -c$((lsite + 1)))
    echo "leaderlogs: repo:$rid#1#f$lsite on $svc, $la to $lb, in the background -> $R/leaderlogs-$TAG.json.xz" >> $R/rows-$TAG.status
    ( python3 tests/fanout/wlleader.py "repo:$rid#1#f$lsite" "$la" "$lb" $R/leaderlogs-$TAG.json.xz "$svc" \
        > $R/leaderlogs-$TAG.log 2>&1; echo "exit $?" >> $R/leaderlogs-$TAG.log ) &
  else
    echo "leaderlogs: no leader site or repository id, not fetched" >> $R/rows-$TAG.status
  fi
fi
echo B033DONE >> $R/rows-$TAG.status
exec sleep infinity
