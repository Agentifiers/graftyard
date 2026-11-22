#!/bin/bash
# mt15's row after the account-stop refusal: the site day (21:49:23Z) and setup (-f1) are done, so only costrow runs.
# ACCT_FROM is today's 00:00Z (the orchestrator's ruling: FRAME counts the account limit per UTC day).
cd <repo>
source /tmp/proxyenv.sh
source tests/fanout/env-m1.sh
RUN=b034mf; L=tests/am1/raw/$RUN-siteday.log
IMG=$(grep -o '@sha256:[0-9a-f]\{8\}' "$L" | head -1 | tr -d @)
echo "$(date -u +%FT%TZ) row restart (ACCT_FROM today)" >> /tmp/$RUN-go.status
export ROWS_ENV=tests/fanout/env-m1.sh
HOSTED=100 ROW=b034 TAG=mt15 ACCT_FROM=2026-11-21T00:00:00Z STOP=170 RUN_FROM=$(cat tests/am1/raw/$RUN.start) \
  RUN_STOP=10 RUN_PREFIX=graft-m1 EXPECT=4 EDGE_LOG=$L IMG=$IMG DRAIN=1 DRAIN_MIN=90 WARM_MIN=45 \
  POOL_PROBE=2:7 B034_DIR=$HOME/b034 COLO_OFF=0.25 ORBCLONE=1 exec bash tests/fanout/costrow.sh
