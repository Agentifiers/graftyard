#!/bin/bash
# The rows' cost stop, between phases (rows.sh) and between L0-12's refusal kinds (rows.mjs, ROWS_COST_CMD). Since the
# operator's 2026-11-16 budget rule the meter is the whole account (tests/fanout/acctcost.py from ACCT_FROM), not one
# install: per-install meters missed most of a $2,328 bill. Appends `cost {...}` and the phase to the status file, and
# exits 1 once the account's spend since ACCT_FROM plus NEXT_USD (the next phase's expected cost, default 0) reaches
# STOP_USD, or the run's own spend since RUN_FROM (default ACCT_FROM) plus NEXT_USD reaches RUN_STOP_USD.
#   ST=<status file> ACCT_FROM=2026-11-16T20:30:00Z STOP_USD=120 [RUN_FROM=<iso> RUN_STOP_USD=25] [NEXT_USD=1]
#     bash tests/fanout/costok.sh <phase>
set -uo pipefail
cd "$(dirname "$0")/../.."
FROM=${ACCT_FROM:?ACCT_FROM is the account meter start, ISO}
usd() { python3 tests/fanout/acctcost.py "$1" 2>/dev/null | head -1 | jq -r '.usd // 999' 2>/dev/null || echo 999; }
# With RUN_PREFIX (graft-m2b) the run's delta counts only that install's scripts, for a slot that shares the account.
runusd() { PREFIX=${RUN_PREFIX:-} python3 tests/fanout/acctcost.py "$1" 2>/dev/null | head -1 | jq -r "${RUN_PREFIX:+.prefix_usd // }.usd // 999" 2>/dev/null || echo 999; }
acct=$(usd "$FROM"); run=$(runusd "${RUN_FROM:-$FROM}")
echo "$(date -u +%FT%TZ) cost account $acct since $FROM, run $run since ${RUN_FROM:-$FROM}, at $1" >> "$ST"
if python3 -c 'import sys; a, r, n, s, rs = map(float, sys.argv[1:]); sys.exit(0 if a + n >= s or r + n >= rs else 1)' \
    "$acct" "$run" "${NEXT_USD:-0}" "${STOP_USD:-120}" "${RUN_STOP_USD:-1e9}"; then
  echo "$(date -u +%FT%TZ) STOP before $1: account \$$acct of stop ${STOP_USD:-120}, run \$$run of stop ${RUN_STOP_USD:-none}, next ${NEXT_USD:-0}" >> "$ST"
  exit 1
fi
