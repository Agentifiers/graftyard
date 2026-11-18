#!/usr/bin/env bash
# m0-carried-5's same-window A/B for B0-02 (`tests/m0/results/carried-5.md`): one group, 16 one-minute runs at 330
# offered, alternating streamed and array member-call bodies (`Host::array_body`, test installs), with the bare-object
# control (`tests/m0carried/gateprobe`) running alongside for the window's platform baseline. Resumes from the raw rows.
#
#   PREFIX=m0c5 bash tests/m0carried/ab5.sh
set -u
cd "$(dirname "$0")/../.."
export EDGE_URL=https://graft-m3c-wp2-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev BENCH_RAW=1
P=${PREFIX:-m0c5}
RAW=tests/m1/wp2/raw
mkdir -p tests/m0carried/raw
# The group: created by the first run, reused by every other.
if [ ! -f $RAW/$P-ab01.bench.json ]; then
  node tests/m1/wp2/run.ts bench --run $P-ab01 --rate 330 --seconds 60 --body stream > /tmp/$P-ab01.log 2>&1
fi
CHAIN=$(jq -r .chain $RAW/$P-ab01.bench.json)
echo "group $CHAIN"
# The control: 50 s probes back to back on its own objects, until the runs end.
T=$(tr -d '\n' < tests/m0/.m0-token)
( n=0; while [ ! -f /tmp/$P-ab.done ]; do n=$((n+1))
    curl -s -m 120 -H "authorization: Bearer $T" \
      "https://graft-m4-gateprobe.${CF_WORKERS_SUBDOMAIN:?}.workers.dev/?modes=nogate,stream,nowrite&seconds=50&bytes=60000&everyMs=150&leader=c5l&peer=c5p&fill=x" \
      > tests/m0carried/raw/$P-probe-$(date -u +%H%M%S).json
  done ) &
for i in $(seq 2 16); do
  run=$(printf "%s-ab%02d" "$P" "$i")
  [ -f $RAW/$run.bench.json ] && continue
  body=$([ $((i % 2)) -eq 1 ] && echo stream || echo array)
  node tests/m1/wp2/run.ts bench --run $run --chain "$CHAIN" --rate 330 --seconds 60 --body $body > /tmp/$run.log 2>&1
  echo "$run $body $(jq -c '[.throughputPerS,.commitMs.p99,.leaderConfirmMs.p99,.roundMs.p50,.leader,.failed,.verdict]' $RAW/$run.bench.json 2>/dev/null)"
done
touch /tmp/$P-ab.done
wait
echo AB_DONE
exec sleep infinity
