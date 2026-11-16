#!/bin/bash
# B0-33 as written on graft-m2b with round 17's instruments, for an orb service: rows.sh's b0-33 row (fresh repo,
# eight learners, k 4, 300 s, 5,000 agents), beside memguard.mjs (every voter's wasmMemMb and memSplit every 2 s,
# SIGTERMs a1.mjs past 120 MiB), terms.mjs (leader timeline) and tsave.mjs (each voter's trace each minute).
#   bash tests/fanout/b033.sh <tag>   -> tests/fanout/raw/{rows,memguard,terms,tsave}-<tag>.*
set -uo pipefail
cd "$(dirname "$0")/../.."
source tests/m3/rows/env.sh
TAG=$1 R=tests/fanout/raw
repo="f$TAG-b033"
node tests/fanout/memguard.mjs "$repo" 1200 2 120 > $R/memguard-$TAG.jsonl 2> $R/memguard-$TAG.err &
node tests/fanout/terms.mjs "$repo" 1200 > $R/terms-$TAG.jsonl 2>&1 &
( while ! rid=$(awk -v n="$repo" '$1==n{print $2}' "$M3_STATE/am1-runs-repos.txt") || [ -z "$rid" ]; do sleep 2; done
  LEARNERS=1 EVERY=10000 DONE_FILE=$R/.tsave-$TAG.done node tests/nr3/tsave.mjs "repo:$rid" > $R/tsave-$TAG.jsonl 2>&1 ) &
T0_FILE=${T0_FILE:-tests/fanout/raw/mem7-t0.t0} ROWS=b0-33 K=4 TEARDOWN=1 NO_LF13=1 STOP_USD=${STOP_USD:-137} LEARNERS=${LEARNERS:-enam:4000,weur:2000,apac:2000} \
  bash tests/fanout/rows.sh "$TAG" &
until grep -q ALLDONE $R/rows-$TAG.status 2>/dev/null; do sleep 5; done
touch $R/.tsave-$TAG.done
sleep 70
echo B033DONE >> $R/rows-$TAG.status
exec sleep infinity
