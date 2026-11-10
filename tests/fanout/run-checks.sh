#!/bin/bash
# The full checks (tests/am1/runs/checks.sh) as an orb service, then its log copied to tests/fanout/raw/checks.log.
cd "$(dirname "$0")/../.."
[ -e tests/fanout/raw/.checks.started ] && exec sleep infinity
touch tests/fanout/raw/.checks.started
mkdir -p tests/am1/raw/runs
bash tests/am1/runs/checks.sh
cp tests/am1/raw/runs/checks.log tests/fanout/raw/checks.log
exec sleep infinity
