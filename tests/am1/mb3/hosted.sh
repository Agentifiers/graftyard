#!/bin/bash
# merge-budget-3's hosted run on graft-m3c after its site day: the agent Apps (m3-admin made a site admin, so the m3-ci
# flag answers 200), a fresh m3org/<repo>, then a B0-35-shaped run at <agents> PRs under App tokens, then teardown of
# this run's shards. Output in tests/am1/raw/mb3/.
#   bash tests/am1/mb3/hosted.sh <repo> <run> [agents] [push-every] [seconds] [extra a1 flags...]
set -uo pipefail
cd "$(dirname "$0")/../../.."
source tests/am1/lf13/env-m3c.sh
REPO=$1 RUN=$2 AGENTS=${3:-50} EVERY=${4:-30} SECS=${5:-300}
shift $(( $# < 5 ? $# : 5 ))
L=tests/am1/raw/mb3/hosted-$RUN.log
mkdir -p tests/am1/raw/mb3
ln -sf $HOME/.graft-m3c/tools.token tests/m0/.m0-token
{ date -u +%FT%TZ; git rev-parse --short HEAD
  node tests/am1/rest/apps.mjs; echo "apps exit $?"
  node tests/am1/runs/mkrepos.mjs "$REPO"; echo "mkrepos exit $?"
  ID=$(awk -v r="$REPO" '$1==r{print $2}' $M3_STATE/am1-runs-repos.txt); echo "repo:$ID"
  node tests/am1/rest/a1.mjs b0-35 --repo "$REPO" --chain repo:$ID --run "$RUN" --agents $AGENTS \
    --push-every $EVERY --seconds $SECS --api-reads 5 --ref-reads 2 --pollers 0 "$@"
  echo "a1 exit $?"; date -u +%FT%TZ; echo DONE; } > "$L" 2>&1
rm -f tests/m0/.m0-token
