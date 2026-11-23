#!/bin/bash
# B0-29 on graft-m4a, for an orb service, as b029-m3f.sh runs it on graft-m3f: b029.sh's row (run 7's script: fresh
# repo, 8 learners, k 4, 500 open PRs, 600 s, 500 agents) with graft-m4a's env (env-m4a.sh: GRAFT_EDGE, the bed and
# tools tokens, M3_STATE and INSTALL under ~/.graft-m4a) and its cost meter by prefix, beside edge500guard.sh on
# graft-m4a-edge. graft-m4a's sites and edge must be test-faults builds (site day 69 and edge day 29 onwards are), since
# the row's learners, k and shard gate use the bed routes. The runner account counts toward graft-m4a's day cap
# (FRAME.md, "a standing row queue"); B0-29 starts no runner, so read it once at the verdict with b035watch.sh's query.
#   RUN_FROM=<site day start> RUN_STOP_USD=10 bash tests/fanout/b029-m4a.sh <tag>
set -uo pipefail
cd "$(dirname "$0")/../.."
export ROWS_ENV=tests/am1/lf13/env-m4a.sh RUN_PREFIX=graft-m4a NODE_USE_ENV_PROXY=1
source $ROWS_ENV
[ "$INSTALL" = graft-m4a ] || { echo "INSTALL is $INSTALL, not graft-m4a" >&2; exit 1; }
[ -s "$BED_TOKEN_FILE" ] && [ -s "$TOOLS_TOKEN_FILE" ] || { echo "no graft-m4a bed or tools token in ~/.graft-m4a" >&2; exit 1; }
R=tests/fanout/raw
[ -e $R/.b029m4a-$1.started ] && exec sleep infinity
touch $R/.b029m4a-$1.started
TAG=$1 SVC=graft-m4a-edge DONE_FILE=$R/rows-$1.status DONE_MARK=ALLDONE bash tests/fanout/edge500guard.sh &
exec bash tests/fanout/b029.sh "$1"
