#!/bin/bash
# narrow-reads-3: the second half of tests/nr3/run.sh on a repository that is already set up (NAME, ID): learners if
# it has none, k.mjs, learner stats, then one B0-29 run of SECONDS_ (default 300), the stats again, and teardown unless
# KEEP=1. FRESH=n runs tests/nr3/fresh.mjs beside it.
#   NAME=nr3-before1 ID=65537 TAG=nr3before bash tests/nr3/load.sh
cd "$(dirname "$0")/../.."
source tests/am1/lf13/env-m3e.sh
ST=tests/nr3/raw/run-$TAG.status
mkdir -p tests/nr3/raw tests/am1/raw/runs
[ -e tests/nr3/raw/.load-$TAG.started ] && exec sleep infinity
touch tests/nr3/raw/.load-$TAG.started
rid=$ID
if [ -z "${NO_LEARNERS:-}" ]; then
  for i in 1 2 3 4 5 6 7 8 9 10; do
    node tests/fanout/learners.mjs "repo:$rid" "${LEARNERS:-enam:4000,weur:2000,apac:2000}" >> "$ST" 2>&1 && break
    echo "$(date -u +%FT%TZ) learners try $i failed" >> "$ST"; sleep 20
  done
fi
node tests/fanout/k.mjs "repo:$rid" >> "$ST" 2>&1
node tests/ld/lstats.mjs "repo:$rid" >> "$ST" 2>&1
echo "$(date -u +%FT%TZ) b0-29 start" >> "$ST"
[ -n "${FRESH:-}" ] && (sleep 60; REPO="$NAME" ROUNDS="$FRESH" node tests/nr3/fresh.mjs > tests/nr3/raw/fresh-$TAG.jsonl 2>&1; echo "$(date -u +%FT%TZ) fresh exit $?" >> "$ST") &
node tests/am1/rest/a1.mjs b0-29 --repo "$NAME" --chain "repo:$rid" --run "$TAG" --seconds "${SECONDS_:-300}" --as-admin \
  > tests/am1/raw/runs/nr-$TAG.log 2>&1
echo "$(date -u +%FT%TZ) b0-29 done $?" >> "$ST"
node tests/ld/lstats.mjs "repo:$rid" >> "$ST" 2>&1
node tests/nr3/lstats.mjs "repo:$rid" >> "$ST" 2>&1
node tests/nr3/trace.mjs "repo:$rid" >> "$ST" 2>&1
wait
[ -z "${KEEP:-}" ] && INSTALL=graft-m3e python3 tests/fanout/teardown.py "$rid" "$rid" > /dev/null 2>&1 && echo "$(date -u +%FT%TZ) torn down $rid" >> "$ST"
echo DONE >> "$ST"
exec sleep infinity
