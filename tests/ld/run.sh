#!/bin/bash
# learner-dumps on graft-m3c, for one orb service: a fresh repository m3org/<NAME> with 500 open PRs (B0-35's setup,
# so its metadata tier is journey 9's size), LEARNERS placed (default 8, weighted as DESIGN says), then one
# B0-29-shaped a1.mjs run of SECONDS (default 300) at the row's 200 API reads/s, its shard containers torn down after.
# Every request runs under the admin PAT (`--as-admin`): m3-admin cannot flag the App on graft-m3c.
#   NAME=ld-b1 TAG=before bash tests/ld/run.sh   -> tests/ld/raw/run-$TAG.status, rows in tests/am1/raw/a1-*.jsonl
cd "$(dirname "$0")/../.."
source tests/am1/lf13/env-m3c.sh
ST=tests/ld/raw/run-$TAG.status
mkdir -p tests/ld/raw tests/am1/raw/runs
[ -e tests/ld/raw/.run-$TAG.started ] && exec sleep infinity
touch tests/ld/raw/.run-$TAG.started
id() { awk -v n="$1" '$1==n{print $2}' "$M3_STATE/am1-runs-repos.txt"; }
node tests/am1/runs/mkrepos.mjs "$NAME" >> "$ST" 2>&1
rid=$(id "$NAME")
[ -n "$rid" ] || { echo "no repo" >> "$ST"; exec sleep infinity; }
echo "$(date -u +%FT%TZ) $NAME = $rid setup" >> "$ST"
# 500 open PRs: B0-35's setup, with a one-second load window and no reads.
node tests/am1/rest/a1.mjs b0-35 --repo "$NAME" --chain "repo:$rid" --run "${TAG}s" --seconds 1 --pollers 0 \
  --api-reads 0 --ref-reads 0 --as-admin > tests/am1/raw/runs/ld-setup-$NAME.log 2>&1
grep -o '"step":"open PRs"[^}]*}' tests/am1/raw/a1-b0-35-${TAG}s.jsonl >> "$ST"
node tests/fanout/learners.mjs "repo:$rid" "${LEARNERS:-enam:4000,weur:2000,apac:2000}" >> "$ST" 2>&1
node tests/fanout/k.mjs "repo:$rid" >> "$ST" 2>&1
node tests/ld/dumpsize.mjs "repo:$rid" 0 2>&1 | head -1 >> "$ST"
echo "$(date -u +%FT%TZ) b0-29 start" >> "$ST"
node tests/am1/rest/a1.mjs b0-29 --repo "$NAME" --chain "repo:$rid" --run "$TAG" --seconds "${SECONDS_:-300}" --as-admin \
  > tests/am1/raw/runs/ld-$TAG.log 2>&1
echo "$(date -u +%FT%TZ) b0-29 done $?" >> "$ST"
INSTALL=graft-m3c python3 tests/fanout/teardown.py "$rid" "$rid" > /dev/null 2>&1 && echo "$(date -u +%FT%TZ) torn down $rid" >> "$ST"
echo DONE >> "$ST"
exec sleep infinity
