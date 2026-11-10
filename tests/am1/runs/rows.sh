#!/bin/bash
# The non-load rows on graft-m2b, one after another, each on its own fresh repository: L0-12's refusals (with the
# install's own App kinds), B0-36 for the confined vendor App, for the Graftyard-run App (shipped, then widened), and
# for `Graftyard MCP`, B0-37's push half, and LF-13 (gap x3, install x3). For one orb service.
#   bash tests/am1/runs/rows.sh
set -uo pipefail
cd "$(dirname "$0")/../../.."
source tests/m3/rows/env.sh
export GRAFT_HOST=${GRAFT_EDGE#https://}
id() { awk -v n="$1" '$1==n{print $2}' "$M3_STATE/am1-runs-repos.txt"; }
O=tests/am1/raw/runs
ROWS_REPO=rows-j ROWS_INSTALL_APPS=1 node tests/am1/hosted/rows.mjs l012 100 > $O/rows-l012.log 2>&1; echo "exit $?" >> $O/rows-l012.log
mv tests/am1/raw/rows-l012.jsonl $O/ 2>/dev/null
for k in m3-ci graftyard-agent graftyard-agent-wide graftyard-mcp; do
  app=$k; [ $k = m3-ci ] && app=; [ $k = graftyard-agent-wide ] && { app=graftyard-agent; node tests/am1/runs/widen.mjs wide >> $O/widen.log 2>&1; sleep 3; }
  ROWS_REPO=rows-j ROWS_B036_APP=$app node tests/am1/hosted/rows.mjs b036 100 > $O/rows-b036-$k.log 2>&1; echo "exit $?" >> $O/rows-b036-$k.log
  mv tests/am1/raw/rows-b036.jsonl $O/rows-b036-$k.jsonl 2>/dev/null
  [ $k = graftyard-agent-wide ] && node tests/am1/runs/widen.mjs shipped >> $O/widen.log 2>&1
done
ROWS_REPO=rows-j ROWS_B037_REPO=rows-b037 ROWS_B037_CHAIN=repo:$(id rows-b037) node tests/am1/hosted/rows.mjs b037 100 > $O/rows-b037.log 2>&1; echo "exit $?" >> $O/rows-b037.log
mv tests/am1/raw/rows-b037.jsonl $O/ 2>/dev/null
for m in gap install; do for r in 1 2 3; do
  repo=lf13-g; [ $m = install ] && repo=lf13-i
  node tests/am1/rest/lf13.mjs $m --repo $repo --chain repo:$(id $repo) > $O/lf13-$m-$r.log 2>&1; echo "exit $?" >> $O/lf13-$m-$r.log
  sleep 20
done; mv tests/am1/raw/lf13-$m.jsonl $O/ 2>/dev/null; done
node tests/am1/runs/l013.mjs demote l013-d > $O/l013-demote.log 2>&1; echo "exit $?" >> $O/l013-demote.log
node tests/am1/runs/l013.mjs plan l013-p 600 > $O/l013-plan.log 2>&1; echo "exit $?" >> $O/l013-plan.log
echo ALLDONE > $O/rows.done
