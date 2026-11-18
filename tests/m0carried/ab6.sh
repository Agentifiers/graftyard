#!/usr/bin/env bash
# m0-carried-6's same-window A/B for B0-02 (`tests/m0/results/carried-6.md`): one group, 16 one-minute runs at 330
# offered, alternating BF-14's arrival rows in SQLite (`sql`, as test installs ran every earlier B0-02) and in memory
# (`mem`, no write outside the entry's transaction), with the bare-object control running alongside. Resumes from the
# raw rows. With GATE=1 it then runs the gate itself: 8 at 300 and 8 at 330 offered, with ARRIVAL (default mem).
#
#   PREFIX=m0c6 bash tests/m0carried/ab6.sh
#   PREFIX=m0c6g GATE=1 ARRIVAL=mem bash tests/m0carried/ab6.sh
set -u
cd "$(dirname "$0")/../.."
export EDGE_URL=${EDGE_URL:-https://graft-m3c-wp2-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev} BENCH_RAW=1
P=${PREFIX:-m0c6}
RAW=tests/m1/wp2/raw
mkdir -p tests/m0carried/raw
T=$(tr -d '\n' < tests/m0/.m0-token)
probe() {
  ( n=0; while [ ! -f /tmp/$P.done ]; do n=$((n+1))
      curl -s -m 120 -H "authorization: Bearer $T" \
        "https://graft-m4-gateprobe.${CF_WORKERS_SUBDOMAIN:?}.workers.dev/?modes=nogate,nowrite&seconds=50&bytes=60000&everyMs=150&leader=${P}l&peer=${P}p&fill=x" \
        > tests/m0carried/raw/$P-probe-$(date -u +%H%M%S).json
    done ) &
}
line() { echo "$1 $2 $(jq -c '[.throughputPerS,.commitMs.p99,.leaderConfirmMs.p99,.roundMs.p50,.leader,.failed,.verdict]' $RAW/$1.bench.json 2>/dev/null)"; }
rm -f /tmp/$P.done
if [ "${GATE:-0}" = 1 ]; then
  # Each run places a new group, as every earlier B0-02 gate run did.
  probe
  for i in 1 2 3 4 5 6 7 8; do
    for rate in 300 330; do
      run=$P-r$rate-$i
      [ -f $RAW/$run.bench.json ] && continue
      node tests/m1/wp2/run.ts bench --run $run --rate $rate --seconds 60 --arrival "${ARRIVAL:-mem}" > /tmp/$run.log 2>&1
      line $run "${ARRIVAL:-mem}"
    done
  done
else
  if [ ! -f $RAW/$P-ab01.bench.json ]; then
    node tests/m1/wp2/run.ts bench --run $P-ab01 --rate 330 --seconds 60 --arrival sql > /tmp/$P-ab01.log 2>&1
  fi
  CHAIN=$(jq -r .chain $RAW/$P-ab01.bench.json)
  echo "group $CHAIN"
  line $P-ab01 sql
  probe
  for i in $(seq 2 16); do
    run=$(printf "%s-ab%02d" "$P" "$i")
    [ -f $RAW/$run.bench.json ] && continue
    mode=$([ $((i % 2)) -eq 1 ] && echo sql || echo mem)
    node tests/m1/wp2/run.ts bench --run $run --chain "$CHAIN" --rate 330 --seconds 60 --arrival $mode > /tmp/$run.log 2>&1
    line $run $mode
  done
fi
touch /tmp/$P.done
wait
echo RUNS_DONE
exec sleep infinity
