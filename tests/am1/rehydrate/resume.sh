#!/bin/bash
# rehydrate-batch's resume after a cut-off rehydrate, hosted: kills repo $HOT_ID's first shard on site 0 and starts its
# rehydrate (tests/am1/rehydrate/cold.mjs with SITES=0), then, CUT_S seconds after the kill (default 120: the
# container's start plus about a minute of the tail), a site-only redeploy of the same build and image
# (tests/am1/rehydrate/siteday.sh with SITE_ONLY=1), which resets the shard object while its container keeps its disk.
# The shard's alarm restarts the run from its saved place; its Workers Logs "shard rehydrate" line then says
# `"resumed": true`. Logs in tests/am1/raw/rehydrate/.
#   bash tests/am1/rehydrate/resume.sh <name>
set -uo pipefail
cd /home/user/workspace/repo
R=tests/am1/raw/rehydrate
echo "resume $1 kill $(date -u +%FT%TZ)" >> $R/timeline.txt
SITES=0 bash tests/am1/rehydrate/run.sh $1-cold node tests/am1/rehydrate/cold.mjs &
sleep ${CUT_S:-120}
echo "resume $1 redeploy $(date -u +%FT%TZ)" >> $R/timeline.txt
SITE_ONLY=1 SKIP_BUILD=--skip-build bash tests/am1/rehydrate/siteday.sh $1-siteday.log
echo "resume $1 redeploy done $(date -u +%FT%TZ)" >> $R/timeline.txt
wait
echo "resume $1 end $(date -u +%FT%TZ)" >> $R/timeline.txt
