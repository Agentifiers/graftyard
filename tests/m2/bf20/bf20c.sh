#!/bin/bash
# bf20c.sh RATE RUNS : BF-20 rerun on repo 196628 (the 3.99 GB tier) after the restore fix, in three phases of RUNS
# runs each: a no-snapshot baseline, the same runs with the writer's metadata snapshot running, and the baseline again,
# so drift in the install shows as a difference between the two baselines. A baseline aborts any running job (a job
# marked running makes `apply` write `snap_undo` copies) and holds every member's snapshot work
# (`/bench_seed {"snap":"hold"}`, test-faults only). The snapshot phase releases the writer W only and forces its
# metadata job. The leader is site 0, as in the first BF-20 run; a run led elsewhere is reported by sum3.py.
# Log: /tmp/bf20/bf20c.log. Runs are c<RATE>a<k>, c<RATE>s<k>, c<RATE>z<k>.
export REPO=196628 CH=repo:196628
R=${1:?rate}; N=${2:?runs}; W=${W:-1}
LOG=/tmp/bf20/bf20c.log
each() { for s in 0 1 2; do up; echo "$(date -u +%FT%TZ) site $s $1: $(/tmp/bf20/m.sh $s /bench_seed "$1")" >> $LOG; done; }
state() { /tmp/bf20/m.sh $W /fold '{"log":true}' | python3 -c "
import json,sys
x=json.load(sys.stdin)['v']; l=[r for r in x['log'] if r['tier']==1]
print('running', x['metadata']['running'], [(r.get('to'), r.get('status'), r.get('rows')) for r in l[:2]])" 2>/dev/null; }
# The orb's egress proxy refuses an edge host for minutes at a time; a run whose load call came back empty is waited out
# and run again. Every call goes through graft-m1-wp7-edge. An idle group's leader steps down and campaigns are
# request-driven, so each run first campaigns site 0 (`LEADER=0` skips load.sh's probe push through graft-m1-edge).
export LEADER=0
up() {
  until [ "$(curl -s -o /dev/null -w '%{http_code}' -m 10 https://graft-m1-wp7-edge.<workers-subdomain>.workers.dev/)" = 401 ]; do sleep 10; done
}
lead() { for t in 1 2 3 4 5; do /tmp/bf20/m.sh 0 /campaign '{}' | grep -q '"verdict":"\(leader\|resumed\|won\)"' && return; sleep 3; done; }
run() {
  for try in 1 2 3; do
    [ -s /tmp/bf20/raw/$1.stats2.json ] && grep -q '"ok"' /tmp/bf20/raw/$1.load.json 2>/dev/null && return
    up; lead; echo "$(date -u +%FT%TZ) start $1 $R/s try $try; $(state)" >> $LOG
    /tmp/bf20/load.sh $1 $R 30 3000 500 1330000 >> $LOG 2>&1; sleep 10
  done
}
baseline() {
  each '{"snap":"hold"}'
  each '{"snap":"abort"}'
  for k in $(seq 1 $N); do run c${R}$1$k; done
}
baseline a
/tmp/bf20/m.sh $W /bench_seed '{"snap":"release"}' >> $LOG
/tmp/bf20/m.sh $W /bench_seed '{"force":1}' >> $LOG
echo "$(date -u +%FT%TZ) forced on $W" >> $LOG
sleep 30
for k in $(seq 1 $N); do run c${R}s$k; done
echo "$(date -u +%FT%TZ) after snapshot phase: $(state)" >> $LOG
baseline z
echo "$(date -u +%FT%TZ) done $R" >> $LOG
