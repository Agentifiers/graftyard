#!/bin/bash
# bf20d.sh RATE : after bf20c.sh, one metadata snapshot of repo 196628 run to its commit under RATE/s: every member's
# snapshot work released, the writer W's job forced, and 30 s runs back to back until the writer's job for the new
# `to` is committed (at most 40 runs). Runs are c<RATE>t<k>. Log: /tmp/bf20/bf20d.log. Then check the object with
# `wrangler r2 object get <bucket>/<key> --remote --pipe | snapcheck metadata`.
export REPO=196628 CH=repo:196628
R=${1:?rate}; W=${W:-1}
LOG=/tmp/bf20/bf20d.log
up() {
  until [ "$(curl -s -o /dev/null -w '%{http_code}' -m 10 https://graft-m1-wp7-edge.<workers-subdomain>.workers.dev/)" = 401 ] &&
    [ "$(curl -s -o /dev/null -w '%{http_code}' -m 10 https://graft-m1-edge.<workers-subdomain>.workers.dev/)" != 000 ]; do sleep 10; done
}
state() { /tmp/bf20/m.sh $W /fold '{"log":true}' | python3 -c "
import json,sys
x=json.load(sys.stdin)['v']; l=[r for r in x['log'] if r['tier']==1]; r=l[0] if l else {}
print(r.get('to'), r.get('status'), r.get('rows'), r.get('batches'), r.get('batchMaxMs'), r.get('key'))" 2>/dev/null; }
for s in 0 1 2; do up; echo "$(date -u +%FT%TZ) site $s: $(/tmp/bf20/m.sh $s /bench_seed '{"snap":"release"}')" >> $LOG; done
up; /tmp/bf20/m.sh $W /bench_seed '{"force":1}' >> $LOG
echo "$(date -u +%FT%TZ) forced on $W" >> $LOG
sleep 30
T0=$(state | cut -d' ' -f1)
echo "$(date -u +%FT%TZ) job to $T0" >> $LOG
for k in $(seq 1 40); do
  up; s=$(state); echo "$(date -u +%FT%TZ) state before c${R}t$k: $s" >> $LOG
  set -- $s
  [ "$2" = committed ] && [ "$1" = "$T0" ] && break
  /tmp/bf20/load.sh c${R}t$k $R 30 3000 500 1330000 >> $LOG 2>&1
  sleep 10
done
echo "$(date -u +%FT%TZ) done: $(state)" >> $LOG
