#!/bin/bash
# B0-29 as written on a test-faults install, for an orb service: rows.sh's b0-29 row (fresh repo, eight learners, k 4
# forced, 500 open PRs, 600 s, 500 agents), beside terms.mjs (leader timeline) and tsave.mjs (each voter's trace, as
# B0-33's runs take it), with the operator's stops checked every 120 s during the row as well as between its phases:
# the account's UTC day past STOP_USD, or this install (RUN_PREFIX) past RUN_STOP_USD since the start (costok.sh).
#   ROWS_ENV=<env> RUN_PREFIX=graft-m2b bash tests/fanout/b029.sh <tag>   -> tests/fanout/raw/{rows,terms,tsave}-<tag>.*
set -uo pipefail
cd "$(dirname "$0")/../.."
source ${ROWS_ENV:-tests/m3/rows/env.sh}
TAG=$1 R=tests/fanout/raw
repo="f$TAG-b029"
export ACCT_FROM=${ACCT_FROM:-$(date -u +%FT00:00:00Z)} STOP_USD=${STOP_USD:-280}
export RUN_FROM=${RUN_FROM:-$(date -u +%FT%TZ)} RUN_STOP_USD=${RUN_STOP_USD:-10} RUN_PREFIX
node tests/fanout/terms.mjs "$repo" 2400 > $R/terms-$TAG.jsonl 2>&1 &
( while ! rid=$(awk -v n="$repo" '$1==n{print $2}' "$M3_STATE/am1-runs-repos.txt") || [ -z "$rid" ]; do sleep 2; done
  LEARNERS=1 ISOLATES=1 EVERY=10000 DONE_FILE=$R/.tsave-$TAG.done node tests/nr3/tsave.mjs "repo:$rid" > $R/tsave-$TAG.jsonl 2>&1 ) &
# The stop during the row: a1.mjs is killed once costok.sh refuses, and rows.sh's next costok stops the rest.
( sleep 60; while ! grep -q ALLDONE $R/rows-$TAG.status 2>/dev/null; do
    ST=$R/rows-$TAG.status bash tests/fanout/costok.sh "b0-29 watch" || { pkill -f "a1.mjs b0-29 --repo $repo " ; break; }
    sleep 120; done ) &
ROWS=b0-29 K=4 TEARDOWN=1 NO_LF13=1 ROW_USD="b0-29:4" LEARNERS=${LEARNERS:-enam:4000,weur:2000,apac:2000} \
  bash tests/fanout/rows.sh "$TAG" &
until grep -q ALLDONE $R/rows-$TAG.status 2>/dev/null; do sleep 5; done
touch $R/.tsave-$TAG.done
sleep 70
# The row's `suspect` reports by edge colo, in its status (tests/fanout/README.md, "Client colos"): SUSPECT withholds
# only on two colos, so a row's leader-change count means something only beside these.
{ echo "colos (voters' Reported rows, whole trace):"; python3 tests/fanout/colos.py $R/tsave-$TAG.jsonl 2000-01-01T00:00:00Z 2100-01-01T00:00:00Z; } >> $R/rows-$TAG.status 2>&1
# The leader's setup-phase confirm beside its load confirm (tests/fanout/README.md, "Placement baseline"); with
# LS_EVERY (a1.mjs --ls-refs-shard-every, through A1_EXTRA), the paired minutes and the isolates too.
{ echo "placement baseline:"; python3 tests/fanout/lsrefsab.py $R/tsave-$TAG.jsonl ${LS_EVERY:-}; } >> $R/rows-$TAG.status 2>&1
echo B029DONE >> $R/rows-$TAG.status
exec sleep infinity
