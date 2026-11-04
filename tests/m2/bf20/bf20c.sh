#!/bin/bash
# bf20c.sh RATE RUNS : BF-20 rerun on repo 196628 (the 3.99 GB tier) after the restore fix, in three phases of RUNS
# runs each: a no-snapshot baseline, the same runs with the writer's metadata snapshot running, and the baseline again,
# so drift in the install shows as a difference between the two baselines. A baseline aborts any running job (a job
# marked running makes `apply` write `snap_undo` copies) and holds every member's snapshot work
# (`/bench_seed {"snap":"hold"}`, test-faults only). The snapshot phase releases the writer W only and forces its
# metadata job. Log: /tmp/bf20/bf20c.log. Runs are c<RATE>a<k>, c<RATE>s<k>, c<RATE>z<k>.
export REPO=196628 CH=repo:196628
R=${1:?rate}; N=${2:?runs}; W=${W:-1}
LOG=/tmp/bf20/bf20c.log
each() { for s in 0 1 2; do echo "$(date -u +%FT%TZ) site $s $1: $(/tmp/bf20/m.sh $s /bench_seed "$1")" >> $LOG; done; }
state() { /tmp/bf20/m.sh $W /fold '{"log":true}' | python3 -c "
import json,sys
x=json.load(sys.stdin)['v']; l=[r for r in x['log'] if r['tier']==1]
print('running', x['metadata']['running'], [(r.get('to'), r.get('status'), r.get('rows')) for r in l[:2]])" 2>/dev/null; }
run() { [ -s /tmp/bf20/raw/$1.stats2.json ] && return; echo "$(date -u +%FT%TZ) start $1 $R/s; $(state)" >> $LOG; /tmp/bf20/load.sh $1 $R 30 3000 500 1330000 >> $LOG 2>&1; sleep 10; }
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
