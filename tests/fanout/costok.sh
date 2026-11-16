#!/bin/bash
# The rows' cost stop, between phases (rows.sh) and between L0-12's refusal kinds (rows.mjs, ROWS_COST_CMD): meters the
# install since T0_FILE (tests/fanout/cost.sh), appends `cost {usd, install, account}` and the phase to the status
# file, and exits 1 once usd plus NEXT_USD (the next phase's expected cost, default 0) reaches STOP_USD. m3fr7's stop was
# checked only before each row, and its L0-12 alone cost $10.42 of the window (`tests/am1/results/lm1-rows-m3f.md`).
#   ST=<status file> STOP_USD=66 [NEXT_USD=13] bash tests/fanout/costok.sh <phase>
set -uo pipefail
cd "$(dirname "$0")/../.."
c=$(bash tests/fanout/cost.sh 2>/dev/null | jq -c '{usd, install, account}' 2>/dev/null)
usd=$(echo "${c:-}" | jq -r '.usd // 99' 2>/dev/null || echo 99)
echo "$(date -u +%FT%TZ) cost ${c:-unread} at $1" >> "$ST"
if python3 -c "import sys; sys.exit(0 if float('$usd') + float('${NEXT_USD:-0}') >= float('${STOP_USD:-12}') else 1)"; then
  echo "$(date -u +%FT%TZ) STOP at \$$usd before $1 (next ${NEXT_USD:-0}, stop ${STOP_USD:-12})" >> "$ST"
  exit 1
fi
