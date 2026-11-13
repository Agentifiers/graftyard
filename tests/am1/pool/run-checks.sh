#!/bin/bash
# The full checks (tests/am1/runs/checks.sh) then the plants (tests/am1/pool/plant.py), as one orb service, into
# tests/am1/raw/pool/checks.log and plants.log.
cd "$(dirname "$0")/../../.."
[ -e tests/am1/raw/pool/.checks.started ] && exec sleep infinity
touch tests/am1/raw/pool/.checks.started
mkdir -p tests/am1/raw/runs
bash tests/am1/runs/checks.sh
cp tests/am1/raw/runs/checks.log tests/am1/raw/pool/checks.log
CARGO_TARGET_DIR=/home/user/workspace/repo/.worktrees/target-rows python3 tests/am1/pool/plant.py > tests/am1/raw/pool/plants.log 2>&1
echo DONE >> tests/am1/raw/pool/plants.log
exec sleep infinity
