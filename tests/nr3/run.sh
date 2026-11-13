#!/bin/bash
# narrow-reads-3 on graft-m3e (from tests/nr/run.sh), for one orb service: learner-dumps' setup (tests/ld/run.sh) on a fresh repository
# m3org/<NAME> with 500 open PRs, 8 learners placed once the group has a leader (retried, since a new group may still be
# electing), then one B0-29 run of SECONDS_ (default 300) at the row's 200 API reads/s, learner /stats before teardown.
#   NAME=nr3-after TAG=after [FRESH=200] bash tests/nr3/run.sh   -> tests/nr3/raw/run-$TAG.status, rows in tests/am1/raw/a1-b0-29-$TAG.jsonl
cd "$(dirname "$0")/../.."
source tests/am1/lf13/env-m3e.sh
ST=tests/nr3/raw/run-$TAG.status
mkdir -p tests/nr3/raw tests/am1/raw/runs
[ -e tests/nr3/raw/.run-$TAG.started ] && exec sleep infinity
touch tests/nr3/raw/.run-$TAG.started
id() { awk -v n="$1" '$1==n{print $2}' "$M3_STATE/am1-runs-repos.txt"; }
for i in 1 2 3 4 5 6; do node tests/am1/runs/mkrepos.mjs "$NAME" >> "$ST" 2>&1 && break; sleep 10; done
rid=$(id "$NAME")
[ -n "$rid" ] || { echo "no repo" >> "$ST"; exec sleep infinity; }
echo "$(date -u +%FT%TZ) $NAME = $rid setup" >> "$ST"
node tests/am1/rest/a1.mjs b0-35 --repo "$NAME" --chain "repo:$rid" --run "${TAG}s" --seconds 1 --pollers 0 \
  --api-reads 0 --ref-reads 0 --as-admin > tests/am1/raw/runs/nr-setup-$NAME.log 2>&1
grep -o '"step":"open PRs"[^}]*}' tests/am1/raw/a1-b0-35-${TAG}s.jsonl >> "$ST"
for i in 1 2 3 4 5 6 7 8 9 10; do
  node tests/fanout/learners.mjs "repo:$rid" "${LEARNERS:-enam:4000,weur:2000,apac:2000}" >> "$ST" 2>&1 && break
  echo "$(date -u +%FT%TZ) learners try $i failed" >> "$ST"; sleep 20
done
node tests/fanout/k.mjs "repo:$rid" >> "$ST" 2>&1
node tests/ld/lstats.mjs "repo:$rid" >> "$ST" 2>&1
node tests/ld/dumpsize.mjs "repo:$rid" 0 2>&1 | head -1 >> "$ST"
echo "$(date -u +%FT%TZ) b0-29 start" >> "$ST"
# FRESH=n: tests/nr3/fresh.mjs's n read-your-write rounds on the same repository, 60 s into the load.
[ -n "${FRESH:-}" ] && (sleep 60; REPO="$NAME" ROUNDS="$FRESH" node tests/nr3/fresh.mjs > tests/nr3/raw/fresh-$TAG.jsonl 2>&1; echo "$(date -u +%FT%TZ) fresh exit $?" >> "$ST") &
node tests/am1/rest/a1.mjs b0-29 --repo "$NAME" --chain "repo:$rid" --run "$TAG" --seconds "${SECONDS_:-300}" --as-admin \
  > tests/am1/raw/runs/nr-$TAG.log 2>&1
echo "$(date -u +%FT%TZ) b0-29 done $?" >> "$ST"
node tests/ld/lstats.mjs "repo:$rid" >> "$ST" 2>&1
node tests/nr3/lstats.mjs "repo:$rid" >> "$ST" 2>&1
node tests/nr3/trace.mjs "repo:$rid" >> "$ST" 2>&1
wait
# KEEP=1 leaves the repository up for tests/nr3/load.sh reruns.
[ -z "${KEEP:-}" ] && INSTALL=graft-m3e python3 tests/fanout/teardown.py "$rid" "$rid" > /dev/null 2>&1 && echo "$(date -u +%FT%TZ) torn down $rid" >> "$ST"
echo DONE >> "$ST"
exec sleep infinity
