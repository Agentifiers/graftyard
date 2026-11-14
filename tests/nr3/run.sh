#!/bin/bash
# narrow-reads-3 on graft-m3e, for one orb service: B0-29 on the shape the operator adopted from LM-1 (lm1-memory.md,
# round 12, "B0-29's window"): a fresh repository m3org/<NAME>, 8 learners and k 4 (K) placed first, then
# `a1.mjs b0-29 --open-prs 500` opens the 500 draft PRs on the agents' own head branches, waits for every learner to
# apply the leader's commit, and runs SECONDS_ (default 600) at the row's rates. Learner /stats before teardown.
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
for i in 1 2 3 4 5 6 7 8 9 10; do
  # A fresh group may sit as three followers holding term 2 after setup (tests/m0/results/f67.md, "The idle shape"):
  # one ordinary request resumes it, as a client's would; learners.mjs's /status polls do not.
  node -e "const {api,tokens}=await import('./tests/am1/hosted/lib.mjs'); await api('GET','/api/v3/repos/m3org/$NAME',tokens.admin)" --input-type=module
  node tests/fanout/learners.mjs "repo:$rid" "${LEARNERS:-enam:4000,weur:2000,apac:2000}" >> "$ST" 2>&1 && break
  echo "$(date -u +%FT%TZ) learners try $i failed" >> "$ST"; sleep 20
done
node tests/fanout/k.mjs "repo:$rid" "${K:-4}" >> "$ST" 2>&1
# ENTRY=max|soft|<bytes>: a diagnostic A/B of the leader's entry cap (test-faults `/stats?entry=`, `Host::entry_soft`),
# set on every voter so a moved leader keeps it. Not a gate row.
[ -n "${ENTRY:-}" ] && for s in 0 1 2; do node -e "const {member}=await import('./tests/am1/hosted/lib.mjs'); const v=(await member('repo:$rid',$s,'/stats?entry=$ENTRY')).v; console.log('entry $ENTRY site $s', v?.entrySoft)" --input-type=module >> "$ST" 2>&1; done
# The leader's memory, log bytes, and held bytes per learner every 15 s from here to the window's end (lmem.mjs).
rm -f tests/nr3/raw/.lmem-$TAG.done
DONE_FILE=tests/nr3/raw/.lmem-$TAG.done node tests/nr3/lmem.mjs "repo:$rid" > tests/nr3/raw/lmem-$TAG.jsonl 2>&1 &
node tests/ld/lstats.mjs "repo:$rid" >> "$ST" 2>&1
node tests/ld/dumpsize.mjs "repo:$rid" 0 2>&1 | head -1 >> "$ST"
echo "$(date -u +%FT%TZ) b0-29 start" >> "$ST"
# FRESH=n: tests/nr3/fresh.mjs's n read-your-write rounds on the same repository, 60 s into the window, which a1.mjs
# starts after its PR opens and the learners' catch-up (its "b0-29 run TAG:" line).
[ -n "${FRESH:-}" ] && (until grep -q "b0-29 run $TAG:" tests/am1/raw/runs/nr-$TAG.log 2>/dev/null; do sleep 5; done; sleep 60; REPO="$NAME" ROUNDS="$FRESH" node tests/nr3/fresh.mjs > tests/nr3/raw/fresh-$TAG.jsonl 2>&1; echo "$(date -u +%FT%TZ) fresh exit $?" >> "$ST") &
node tests/am1/rest/a1.mjs b0-29 --repo "$NAME" --chain "repo:$rid" --run "$TAG" --seconds "${SECONDS_:-600}" --open-prs "${OPEN_PRS:-500}" --as-admin \
  > tests/am1/raw/runs/nr-$TAG.log 2>&1
echo "$(date -u +%FT%TZ) b0-29 done $?" >> "$ST"
touch tests/nr3/raw/.lmem-$TAG.done
grep -hE '"step":"(open PRs|learners caught up)"' tests/am1/raw/a1-b0-29-$TAG.jsonl >> "$ST" 2>/dev/null
node tests/ld/lstats.mjs "repo:$rid" >> "$ST" 2>&1
node tests/nr3/lstats.mjs "repo:$rid" >> "$ST" 2>&1
node tests/nr3/trace.mjs "repo:$rid" >> "$ST" 2>&1
node tests/nr3/egress.mjs "repo:$rid" >> "$ST" 2>&1
RAW=tests/nr3/raw/trace-$TAG.json node tests/nr3/split.mjs "repo:$rid" >> "$ST" 2>&1
wait
# KEEP=1 leaves the repository up for tests/nr3/load.sh reruns.
[ -z "${KEEP:-}" ] && INSTALL=graft-m3e python3 tests/fanout/teardown.py "$rid" "$rid" > /dev/null 2>&1 && echo "$(date -u +%FT%TZ) torn down $rid" >> "$ST"
echo DONE >> "$ST"
exec sleep infinity
