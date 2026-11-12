#!/bin/bash
# lf4-rest-2's hosted sequence on graft-m3c, after tests/am1/lf4/siteday.sh: the m3 admin made a site admin
# (tests/am1/rest/apps.mjs, idempotent), a fresh repository, LF-4 on all eight resources through the edge's routes,
# the hosted capture of the new POSTs for validate.py, B0-36's labels through the site-admin flag route, and the
# shard teardown. Each step writes a stamp under tests/am1/raw/lf4/h2/, so a restart resumes after the last done step.
#   bash tests/am1/lf4/hosted2.sh [run] [repo]   (waits for siteday<run>.log's DONE first; default run 2, lf4-t)
# lf4-rest-3 reran it as `3 lf4-c3`: LF-4 with the check runs created concurrently, the capture, and the teardown,
# without B0-36.
set -uo pipefail
cd /home/user/workspace/repo
source tests/am1/lf13/env-m3c.sh
RUN=${1:-2}; REPO=${2:-lf4-t}
D=tests/am1/raw/lf4/h$RUN
mkdir -p "$D"
ln -sf ~/.graft-m3c/tools.token tests/m0/.m0-token
step() { local n=$1; shift; [ -f "$D/$n.done" ] && return 0; echo "== $n $(date -u +%TZ)"; "$@" > "$D/$n.log" 2>&1; local rc=$?; echo "$n rc=$rc $(date -u +%TZ)"; [ $rc -eq 0 ] && touch "$D/$n.done"; return $rc; }
S=tests/am1/raw/lf4/siteday$RUN.log
until grep -q '^DONE' $S 2>/dev/null; do sleep 20; done
grep -q 'site exit 0' $S && grep -q 'edge exit 0' $S || { echo "deploy failed"; exit 1; }
{
step apps node tests/am1/rest/apps.mjs || exit 1
if [ $RUN = 2 ]; then step repos node tests/am1/runs/mkrepos.mjs $REPO b036-m || exit 1
else step repos node tests/am1/runs/mkrepos.mjs $REPO || exit 1; fi
ID=$(awk -v r=$REPO '$1==r{print $2}' "$M3_STATE/am1-runs-repos.txt")
step lf4 node tests/am1/rest/lf4.mjs --repo $REPO --chain repo:$ID || exit 1
TAG=$(tail -1 tests/am1/raw/lf4.jsonl | python3 -c 'import json,sys; print(json.load(sys.stdin)["tag"])')
TIP=$(node -e "import('./tests/am1/hosted/lib.mjs').then(({Git,tokens})=>console.log(new Git('m3org/$REPO').as(tokens.admin).remoteHead('refs/heads/lf4/$TAG/commits')))")
rm -rf /tmp/hosted-replies$RUN /tmp/hosted-replies$RUN-refusals
step capture node tests/am1/lf4/capture.mjs --repo $REPO --out /tmp/hosted-replies$RUN --tip "$TIP" || exit 1
step validate env DOTCOM_OPENAPI=/tmp/ghes/api.github.com.json /tmp/ghes/.venv/bin/python crates/api/gh/validate.py /tmp/ghes/ghes-3.22.json /tmp/ghes/schema.docs-enterprise.graphql /tmp/hosted-replies$RUN
[ $RUN = 2 ] && step b036 node tests/am1/lf4/b036-labels.mjs --repo b036-m --n 100
step teardown python3 tests/fanout/teardown.py --all-nets --pools
rm -f tests/m0/.m0-token
echo HDONE
} >> "$D/run.log" 2>&1
