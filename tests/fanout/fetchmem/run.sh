#!/usr/bin/env bash
# Peak workerd RSS while 1 and then 4 R2 bodies of SIZE bytes stream to a sink at RATE bytes/s each (README.md).
set -euo pipefail
cd "$(dirname "$0")"
W=${WRANGLER:-/home/user/workspace/repo/tests/m0/node_modules/.bin/wrangler}
SIZE=${SIZE:-207000000}
RATE=${RATE:-60000000} python3 sink.py 8791 & SINK=$!
dev() { "$W" dev --port 8790 --persist-to /tmp/fetchmem-state >/tmp/fetchmem-dev.log 2>&1 & DEV=$!
  until curl -s http://127.0.0.1:8790/nope 2>/dev/null | grep -q "not found"; do sleep 1; done; }
trap 'kill $SINK $DEV 2>/dev/null || true' EXIT
# Seeding grows workerd's heap, so measure in a fresh workerd over the persisted bucket.
dev; curl -s "http://127.0.0.1:8790/seed?n=${SEED:-4}&size=$SIZE"; kill $DEV; wait $DEV 2>/dev/null || true; sleep 2; dev
wd() { # the workerd process with the most memory: the one running the isolate
  for p in $(pgrep -x workerd); do echo "$(awk '/VmRSS/{print $2}' /proc/$p/status) $p"; done | sort -n | tail -1 | cut -d' ' -f2; }
peak() { # peak RSS in MB of workerd over the call
  local pid; pid=$(wd); local max=0
  "$@" >/tmp/fetchmem-out.json & local c=$!
  while kill -0 $c 2>/dev/null; do r=$(awk '/VmRSS/{print $2}' /proc/$pid/status); ((r > max)) && max=$r; sleep 0.2; done
  echo "$(cat /tmp/fetchmem-out.json) rss_peak_mb=$((max / 1024)) rss_now_mb=$(( $(awk '/VmRSS/{print $2}' /proc/$pid/status) / 1024 ))"
}
echo "idle rss_mb=$(( $(awk '/VmRSS/{print $2}' /proc/$(wd)/status) / 1024 ))"
# QS: the queries, space-separated (default: 1 and 4 streamed, twice, then the buffered control); SEED: packs seeded.
for q in ${QS:-k=1 k=4 k=1 k=4 k=1&mode=buffer k=4&mode=buffer}; do peak curl -s "http://127.0.0.1:8790/fetch?$q&size=$SIZE"; done
