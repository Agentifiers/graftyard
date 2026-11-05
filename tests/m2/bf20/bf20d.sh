#!/bin/bash
# bf20d.sh RATE : after bf20c.sh, one metadata snapshot of repo 196628 run to its commit under RATE/s: every member's
# snapshot work released, the writer W's job forced, and 30 s runs back to back until the writer's job for the new
# `to` is committed (at most 40 runs). Runs are c<RATE>t<k>. Log: $B/bf20d.log. Then check the object with
# `wrangler r2 object get <bucket>/<key> --remote --pipe | snapcheck metadata`.
B=${BF20_DIR:-/tmp/bf20}; export REPO=${REPO:-196628}; export CH=repo:$REPO
R=${1:?rate}; W=${W:-1}
LOG=$B/bf20d.log
# As in bf20c.sh: every call through graft-m1-wp7-edge, and site 0 campaigned before each run.
export LEADER=0
up() {
  until [ "$(curl -s -o /dev/null -w '%{http_code}' -m 10 ${GRAFT_TOOLS_EDGE:-https://graft-m1-wp7-edge.<workers-subdomain>.workers.dev}/)" = 401 ]; do sleep 10; done
}
lead() { for t in 1 2 3 4 5; do $B/m.sh 0 /campaign '{}' | grep -q '"verdict":"\(leader\|resumed\|won\)"' && return; sleep 3; done; }
state() { $B/m.sh $W /fold '{"log":true}' | python3 -c "
import json,sys
x=json.load(sys.stdin)['v']; l=[r for r in x['log'] if r['tier']==1]; r=l[0] if l else {}
print(r.get('to'), r.get('status'), r.get('rows'), r.get('batches'), r.get('batchMaxMs'), r.get('key'))" 2>/dev/null; }
# Only the writer is released, as in bf20c.sh's snapshot phase; the others stay held.
for s in 0 1 2; do up; [ $s = $W ] && a=release || a=hold; echo "$(date -u +%FT%TZ) site $s: $($B/m.sh $s /bench_seed "{\"snap\":\"$a\"}")" >> $LOG; done
up; $B/m.sh $W /bench_seed '{"force":1}' >> $LOG
echo "$(date -u +%FT%TZ) forced on $W" >> $LOG
sleep 30
T0=$(state | cut -d' ' -f1)
echo "$(date -u +%FT%TZ) job to $T0" >> $LOG
for k in $(seq 1 40); do
  up; s=$(state); echo "$(date -u +%FT%TZ) state before c${R}t$k: $s" >> $LOG
  set -- $s
  [ "$2" = committed ] && [ "$1" = "$T0" ] && break
  lead; $B/load.sh c${R}t$k $R 30 3000 500 1330000 >> $LOG 2>&1
  sleep 10
done
echo "$(date -u +%FT%TZ) done: $(state)" >> $LOG
