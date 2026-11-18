#!/usr/bin/env bash
# m0-carried-7's entry-cap A/B for B0-02 (`tests/m0/results/carried-7.md`): 16 one-minute runs at 330 offered, each on
# a new group, alternating entries capped at ENTRY_SOFT (`soft`) and filled to ENTRY_MAX as before (`max`), all with
# BF-14's arrival rows in memory. A and B name the two arms' `--entry` (default soft and max), such as A=soft B=80000.
# Resumes from the raw rows. With GATE=1 it runs the gate instead: 8 at 300 and 8 at
# 330 offered, each on a new group, capped.
#
#   PREFIX=m0c7 bash tests/m0carried/ab7.sh
#   PREFIX=m0c7b A=soft B=80000 bash tests/m0carried/ab7.sh
#   PREFIX=m0c7g GATE=1 bash tests/m0carried/ab7.sh
set -u
cd "$(dirname "$0")/../.."
export EDGE_URL=${EDGE_URL:-https://graft-m3d-wp2-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev} BENCH_RAW=1
P=${PREFIX:-m0c7}
RAW=tests/m1/wp2/raw
line() { echo "$1 $2 $(jq -c '[.throughputPerS,.commitMs.p99,.leaderConfirmMs.p99,.commandsPerEntry,.entries,.leader,.failed,.verdict]' $RAW/$1.bench.json 2>/dev/null)"; }
bench() { node tests/m1/wp2/run.ts bench --run "$1" --rate "$2" --seconds 60 --arrival mem --entry "$3" > /tmp/$1.log 2>&1; line "$1" "$3"; }
if [ "${GATE:-0}" = 1 ]; then
  for i in 1 2 3 4 5 6 7 8; do
    for rate in 300 330; do
      [ -f $RAW/$P-r$rate-$i.bench.json ] || bench $P-r$rate-$i $rate soft
    done
  done
else
  for i in $(seq 1 16); do
    run=$(printf "%s-ab%02d" "$P" "$i")
    [ -f $RAW/$run.bench.json ] || bench $run 330 $([ $((i % 2)) -eq 1 ] && echo ${A:-soft} || echo ${B:-max})
  done
fi
echo RUNS_DONE
exec sleep infinity
