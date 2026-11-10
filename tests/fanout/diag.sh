#!/bin/bash
# Like-for-like with am1-runs' diagnosis runs (`diag-2` at 100 agents and `a1-smoke` at 500, reads and pollers off):
# fdiag2 at 100 agents, k = 1; fdiag5 at 500 agents, k = 4. For one orb service.
cd "$(dirname "$0")/../.."
source tests/m3/rows/env.sh
[ -e tests/fanout/raw/.diag.started ] && exec sleep infinity
touch tests/fanout/raw/.diag.started
id() { awk -v n="$1" '$1==n{print $2}' "$M3_STATE/am1-runs-repos.txt"; }
bash tests/am1/runs/a1.sh b0-29 fdiag2 "$(id fdiag2)" --agents 100 --seconds 180 --api-reads 0 --ref-reads 0 --pollers 0
node tests/fanout/k.mjs "repo:$(id fdiag5)" 4 > tests/fanout/raw/diag.status 2>&1
bash tests/am1/runs/a1.sh b0-29 fdiag5 "$(id fdiag5)" --agents 500 --seconds 300 --api-reads 0 --ref-reads 0 --pollers 0
node tests/fanout/k.mjs "repo:$(id fdiag5)" >> tests/fanout/raw/diag.status 2>&1
echo DONE >> tests/fanout/raw/diag.status
exec sleep infinity
