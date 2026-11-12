#!/bin/bash
# hot-shards' journey-9 phase on graft-m3c, once the seed has built the log above the last `Promoted`: release the
# archive hold (so promote-ahead and archiving run on the design's 2 GiB and 2 MiB), then push at 4.17/s for SECONDS
# (default 3600) while tests/am1/hotshards/faults.mjs transfers leadership twice and kills the leader's shard once.
# The monitor (tests/am1/hotshards/monitor.mjs) keeps sampling throughout. Logs go to tests/am1/raw/hotshards/.
#   bash tests/am1/hotshards/j9.sh
set -uo pipefail
cd /home/user/workspace/repo
R=tests/am1/raw/hotshards
touch $R/UNHOLD
sleep 20
echo "j9 start $(date -u +%FT%TZ)" >> $R/timeline.txt
mv $R/pushes.jsonl $R/seed-pushes.jsonl 2>/dev/null
bash tests/am1/hotshards/run.sh faults env AT="${AT:-10:transfer,25:kill,40:transfer}" node tests/am1/hotshards/faults.mjs &
MODE=j9 PUSH_PER_S=4.17 INFLIGHT=${INFLIGHT:-16} SECONDS=${SECONDS_J9:-3600} bash tests/am1/hotshards/run.sh j9 node tests/am1/hotshards/load.mjs
wait
echo "j9 end $(date -u +%FT%TZ)" >> $R/timeline.txt
