#!/bin/bash
# b030-split smoke 2 on graft-m1: a fresh repository m3org/b030-s2 seeded to TARGET (default 400 MB, a tenth of
# B0-30's tier, so the split costs a tenth of smoke 1's), then smoke2.mjs.
#   amp orb service start b030split-smoke2 --command 'bash tests/am1/b030split/smoke2.sh'
# Smoke 3 (after SLIM_BYTES): NAME=b030-s3 TAG=smoke3, which log to b030split-smoke3.{log,jsonl}.
cd "$(dirname "$0")/../../.."
export BED_TOKEN_FILE=$HOME/.graft-wp6/bed.token
TAG=${TAG:-smoke2}; export TAG
L=tests/am1/raw/b030split-$TAG.log
{
  date -u +%FT%TZ
  node tests/am1/b030/mkrepo.mjs "${NAME:-b030-s2}" || exit 1
  [ -s tests/am1/raw/b030-seed.json ] && grep -q "\"repo:$(jq .repo tests/am1/raw/b030-repo.json)\"" tests/am1/raw/b030-seed.json \
    || node tests/am1/b030/seed.mjs "${TARGET:-400000000}" || exit 1
  node tests/am1/b030split/smoke2.mjs
  echo "exit $?"; date -u +%FT%TZ; echo DONE
} >> "$L" 2>&1
exec sleep infinity
