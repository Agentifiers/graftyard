#!/bin/bash
# B0-29 on graft-m3f, for an orb service, through the egress proxy (tests/fanout/README.md, "Harness outages on gate
# rows"): b029.sh's row (run 7's script: fresh repo, 8 learners, k 4, 500 open PRs, 600 s, 500 agents) with the install's
# env and its cost meter, beside edge500guard.sh, which stops the row at the edge's first 500 or exception.
#   RUN_FROM=<site day start> RUN_STOP_USD=6 bash tests/fanout/b029-m3f.sh <tag>
set -uo pipefail
cd "$(dirname "$0")/../.."
export ROWS_ENV=tests/am1/lf13/env-m3f.sh RUN_PREFIX=graft-m3f NODE_USE_ENV_PROXY=1
source $ROWS_ENV
R=tests/fanout/raw
[ -e $R/.b029m3f-$1.started ] && exec sleep infinity
touch $R/.b029m3f-$1.started
TAG=$1 SVC=graft-m3f-edge DONE_FILE=$R/rows-$1.status DONE_MARK=ALLDONE bash tests/fanout/edge500guard.sh &
exec bash tests/fanout/b029.sh "$1"
