#!/bin/bash
# B0-35 on graft-m4a with Actions dispatch (`decisions/DECISIONS.md`, "`pr-trigger`"), as one orb service, after site
# day 51 set GRAFTYARD_PR_TRIGGER: a fresh repository with LM-1's 8 learners (enam 4, weur 2, apac 2; k stays 1, since
# k.mjs's `/bench_seed` is test-faults only), then `a1.mjs b0-35 --workflow --as-admin` (500 open PRs, 600 s,
# 500 `mergeable` pollers every 5 s; graft-m4a's product edge mints no task-scoped App tokens for a confined fleet, so
# pushes are Human class, as LM-1's B0-35 rounds on product builds ran), then the teardown sweep through the scratch
# shardkill Worker. The stop watcher (b035watch.sh), the voter watch (b035lead.sh) and, on a test-faults build, the
# trace saver (b035tsave.sh) run beside it, each as its own service with the same TAG.
#   TAG=<tag> bash tests/m4/wp4/b035.sh   -> tests/m4/raw/wp4/b035-<tag>.status
set -uo pipefail
cd "$(dirname "$0")/../../.."
export ROWS_ENV=tests/am1/lf13/env-m4a.sh
source $ROWS_ENV
[ "$INSTALL" = graft-m4a ] || { echo "INSTALL is $INSTALL, not graft-m4a" >&2; exit 1; }
ST=tests/m4/raw/wp4/b035-$TAG.status
mkdir -p tests/m4/raw/wp4 tests/am1/raw/runs
[ -e tests/m4/raw/wp4/.b035-$TAG.started ] && exec sleep infinity
[ -z "$(git status --porcelain --untracked-files=no)" ] || { echo "b035.sh: commit the driver changes first" >&2; exit 1; }
touch tests/m4/raw/wp4/.b035-$TAG.started
echo "$(date -u +%FT%TZ) start $(git rev-parse --short HEAD)" >> "$ST"
repo="b035-$TAG"
node tests/am1/runs/mkrepos.mjs "$repo" >> "$ST" 2>&1
rid=$(awk -v n="$repo" '$1==n{print $2}' "$M3_STATE/am1-runs-repos.txt")
[ -n "$rid" ] || { echo "$(date -u +%FT%TZ) no repo" >> "$ST"; echo ALLDONE >> "$ST"; exec sleep infinity; }
node tests/fanout/learners.mjs "repo:$rid" enam:4000,weur:2000,apac:2000 >> "$ST" 2>&1 \
  || echo "$(date -u +%FT%TZ) LEARNERS NOT PLACED; the row runs on its followers" >> "$ST"
echo "$(date -u +%FT%TZ) b0-35 $repo $rid start" >> "$ST"
bash tests/am1/runs/a1.sh b0-35 "$repo" "$rid" --seconds 600 --workflow --as-admin
echo "$(date -u +%FT%TZ) b0-35 $repo $rid done $(grep -E '^exit' tests/am1/raw/runs/a1-b0-35-$repo.log)" >> "$ST"
if python3 tests/fanout/teardown.py --sweep --shardkill >> "$ST" 2>&1; then echo "$(date -u +%FT%TZ) sweep ok" >> "$ST"
else echo "$(date -u +%FT%TZ) SWEEP FAILED" >> "$ST"; fi
echo ALLDONE >> "$ST"
exec sleep infinity
