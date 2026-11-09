#!/bin/bash
# The A1 load rows on graft-m2b, one after another, each on its own fresh repository (tests/am1/runs/mkrepos.mjs):
# B0-31 (leader moved to site 2, apac, first), B0-32, B0-34, B0-35, L0-12, B0-33. For one orb service.
#   bash tests/am1/runs/loadrows.sh
set -uo pipefail
cd "$(dirname "$0")/../../.."
source tests/m3/rows/env.sh
id() { awk -v n="$1" '$1==n{print $2}' "$M3_STATE/am1-runs-repos.txt"; }
node tests/am1/runs/transfer.mjs "repo:$(id a1-31)" 2 > tests/am1/raw/runs/transfer-a1-31.log 2>&1
bash tests/am1/runs/a1.sh b0-31 a1-31 "$(id a1-31)" --seconds 600
bash tests/am1/runs/a1.sh b0-32 a1-32 "$(id a1-32)" --seconds 180
bash tests/am1/runs/a1.sh b0-34 a1-34 "$(id a1-34)" --seconds 600 --kill-shard-at 300
bash tests/am1/runs/a1.sh b0-35 a1-35 "$(id a1-35)" --seconds 600
bash tests/am1/runs/a1.sh l0-12 a1-l012 "$(id a1-l012)" --seconds 600 --kill-every 60
bash tests/am1/runs/a1.sh b0-33 a1-33 "$(id a1-33)" --seconds 300
echo ALLDONE > tests/am1/raw/runs/loadrows.done
