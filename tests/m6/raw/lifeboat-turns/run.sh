#!/bin/bash
# One stale-bed run: bash run.sh <16|64> <seconds>, from the repository root, with BF-53's lifeboat state copied to
# /tmp/lbm/state0, the bundle's builds under /tmp/bf53/builds, and c16.capnp and c64.capnp in /tmp/lbm: BF-53's
# config.capnp with FEED_URL and EDGE_URL on the stand-in (127.0.0.1:8600) and the lifeboat build at AT_ONCE 16 or 64. Each run starts
# from a fresh copy of that state (3,236 chains), with the feed stopped and the edge stubbed with BF-53's hosted edge wall
# times (`edge-wall-bf53.json.xz`, decompressed to /tmp/lbm/edge_wall.json); it logs the puller's pass lines and
# /status. `python3 an.py /tmp/lbm/run<n>` prints the pass time and cycles.
set -u
root=$(pwd)
n=$1; secs=$2; d=/tmp/lbm/run$n
rm -rf "$d" /tmp/lbm/state; mkdir -p "$d"; cp -a /tmp/lbm/state0 /tmp/lbm/state
node "$root/tests/m6/raw/lifeboat-turns/stub.mjs" 8600 /tmp/lbm/edge_wall.json 30 >"$d/stub.log" 2>&1 & S=$!
ulimit -n 524288
cd /tmp/lbm && { "$root"/tests/m0/node_modules/.bin/workerd serve c$n.capnp --experimental >"$d/workerd.log" 2>&1 & W=$!; }
for _ in $(seq 60); do curl -s -m 2 -o /dev/null http://127.0.0.1:8602/status && break; sleep 0.5; done
t0=$(date +%s)
while [ $(( $(date +%s) - t0 )) -lt "$secs" ]; do
  printf '%s %s\n' "$(date +%s%3N)" "$(curl -s -m 5 http://127.0.0.1:8602/status | python3 -c 'import json,sys; s=json.load(sys.stdin); print(s["current_at"], len(s["held"]), s["pages"])' 2>/dev/null)" >>"$d/status.log"
  sleep 2
done
kill $W $S; wait 2>/dev/null
echo "done $n"
