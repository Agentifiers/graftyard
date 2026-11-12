#!/bin/bash
# lf4-rest-2's hosted sequence on graft-m3c, after tests/am1/lf4/siteday.sh: the m3 admin made a site admin
# (tests/am1/rest/apps.mjs, idempotent), a fresh repository, LF-4 on all eight resources through the edge's routes,
# the hosted capture of the new POSTs for validate.py, B0-36's labels through the site-admin flag route, and the
# shard teardown. Each step writes a stamp under tests/am1/raw/lf4/h2/, so a restart resumes after the last done step.
#   bash tests/am1/lf4/hosted2.sh   (waits for siteday2.log's DONE first)
set -uo pipefail
cd /home/user/workspace/repo
source tests/am1/lf13/env-m3c.sh
D=tests/am1/raw/lf4/h2
mkdir -p "$D"
ln -sf ~/.graft-m3c/tools.token tests/m0/.m0-token
step() { local n=$1; shift; [ -f "$D/$n.done" ] && return 0; echo "== $n $(date -u +%TZ)"; "$@" > "$D/$n.log" 2>&1; local rc=$?; echo "$n rc=$rc $(date -u +%TZ)"; [ $rc -eq 0 ] && touch "$D/$n.done"; return $rc; }
until grep -q '^DONE' tests/am1/raw/lf4/siteday2.log 2>/dev/null; do sleep 20; done
grep -q 'site exit 0' tests/am1/raw/lf4/siteday2.log && grep -q 'edge exit 0' tests/am1/raw/lf4/siteday2.log || { echo "deploy failed"; exit 1; }
{
step apps node tests/am1/rest/apps.mjs || exit 1
step repos node tests/am1/runs/mkrepos.mjs lf4-t b036-m || exit 1
ID=$(awk '$1=="lf4-t"{print $2}' "$M3_STATE/am1-runs-repos.txt")
step lf4 node tests/am1/rest/lf4.mjs --repo lf4-t --chain repo:$ID || exit 1
TAG=$(tail -1 tests/am1/raw/lf4.jsonl | python3 -c 'import json,sys; print(json.load(sys.stdin)["tag"])')
TIP=$(node -e "import('./tests/am1/hosted/lib.mjs').then(({Git,tokens})=>console.log(new Git('m3org/lf4-t').as(tokens.admin).remoteHead('refs/heads/lf4/$TAG/commits')))")
rm -rf /tmp/hosted-replies2
step capture node tests/am1/lf4/capture.mjs --repo lf4-t --out /tmp/hosted-replies2 --tip "$TIP" || exit 1
step validate env DOTCOM_OPENAPI=/tmp/ghes/api.github.com.json /tmp/ghes/.venv/bin/python crates/api/gh/validate.py /tmp/ghes/ghes-3.22.json /tmp/ghes/schema.docs-enterprise.graphql /tmp/hosted-replies2
step b036 node tests/am1/lf4/b036-labels.mjs --repo b036-m --n 100
step teardown python3 tests/fanout/teardown.py --all-nets --pools
rm -f tests/m0/.m0-token
echo HDONE
} >> "$D/run.log" 2>&1
