#!/bin/bash
# rehydrate-batch's drivers on graft-m3f, as tests/am1/hotshards/run.sh runs them on graft-m3c: the graft-m3f env, the
# tools token linked where tests/am1/hosted/lib.mjs reads it (git-ignored), and HOT_REPO, HOT_CHAIN, HOT_ID from
# tests/am1/raw/rehydrate/repo. The hot-shards drivers write their rows under tests/am1/raw/hotshards/; output goes to
# tests/am1/raw/rehydrate/<name>.log.
#   bash tests/am1/rehydrate/run.sh <name> <command...>
set -uo pipefail
cd /home/user/workspace/repo
source tests/am1/lf13/env-m3f.sh
ln -sf $HOME/.graft-m3f/tools.token tests/m0/.m0-token
mkdir -p tests/am1/raw/rehydrate tests/am1/raw/hotshards
if [ -f tests/am1/raw/rehydrate/repo ]; then
  read -r name id < tests/am1/raw/rehydrate/repo
  export HOT_REPO=m3org/$name HOT_CHAIN=repo:$id HOT_ID=$id
fi
name=$1; shift
"$@" >> tests/am1/raw/rehydrate/$name.log 2>&1
echo "exit $? $(date -u +%FT%TZ)" >> tests/am1/raw/rehydrate/$name.log
