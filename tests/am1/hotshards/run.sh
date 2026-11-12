#!/bin/bash
# Runs one hot-shards driver against graft-m3c with its environment: the graft-m3c env, the tools token linked where
# tests/am1/hosted/lib.mjs reads it (git-ignored), and HOT_REPO, HOT_CHAIN, HOT_ID from tests/am1/raw/hotshards/repo.
# Output goes to tests/am1/raw/hotshards/<name>.log.
#   bash tests/am1/hotshards/run.sh <name> <command...>
set -uo pipefail
cd /home/user/workspace/repo
source tests/am1/lf13/env-m3c.sh
ln -sf $HOME/.graft-m3c/tools.token tests/m0/.m0-token
if [ -f tests/am1/raw/hotshards/repo ]; then
  read -r name id < tests/am1/raw/hotshards/repo
  export HOT_REPO=m3org/$name HOT_CHAIN=repo:$id HOT_ID=$id
fi
name=$1; shift
"$@" >> tests/am1/raw/hotshards/$name.log 2>&1
echo "exit $? $(date -u +%FT%TZ)" >> tests/am1/raw/hotshards/$name.log
