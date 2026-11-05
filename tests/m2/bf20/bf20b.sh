#!/bin/bash
# bf20b.sh : BF-20 on repo 196628 (bf20b), resumable. Baselines at 150/s and 300/s, then the metadata snapshot forced
# on the writer with the same runs back to back until it commits, then baselines again. Log: $B/bf20b.log.
B=${BF20_DIR:-/tmp/bf20}; export REPO=${REPO:-196628}; export CH=repo:$REPO
LOG=$B/bf20b.log
run() { [ -s $B/raw/$1.stats2.json ] && return; echo "$(date -u +%FT%TZ) start $1 $2/s" >> $LOG; $B/load.sh $1 $2 30 3000 500 1330000 >> $LOG 2>&1; sleep 15; }
state() { $B/m.sh $1 /fold '{"log":true}' | python3 -c "
import json,sys
x=json.load(sys.stdin)['v']; l=[r for r in x['log'] if r['tier']==1]; r=l[0] if l else {}
print(x['metadata']['applied'], x['metadata']['running'], r.get('to'), r.get('status'), r.get('batches'), r.get('batchMaxMs'), r.get('rows'), r.get('bytes'))" 2>/dev/null; }
for n in pre150a pre300a pre150b pre300b; do run $n ${n:3:3}; done
W=$(cat $B/bf20b.writer 2>/dev/null)
if [ -z "$W" ]; then
  W=1; echo $W > $B/bf20b.writer
  $B/m.sh $W /bench_seed '{"force":1}' >> $LOG
  echo "$(date -u +%FT%TZ) forced on $W" >> $LOG
fi
k=0
while :; do
  k=$((k+1)); s=$(state $W); echo "$(date -u +%FT%TZ) snap state before run $k: $s" >> $LOG
  set -- $s
  [ "$4" = committed ] && [ "${3:-0}" -gt 10 ] 2>/dev/null && break
  [ $k -gt 24 ] && break
  if [ $((k % 2)) = 1 ]; then run snap150-$k 150; else run snap300-$k 300; fi
done
for n in post150a post300a; do run $n ${n:4:3}; done
echo "$(date -u +%FT%TZ) done" >> $LOG
sleep 100000
