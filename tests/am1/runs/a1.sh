#!/bin/bash
# One a1.mjs run on graft-m2b, for an orb service: `bash tests/am1/runs/a1.sh <row> <repo> <id> [a1.mjs flags...]`.
# Output in tests/am1/raw/runs/a1-<row>-<repo>.log; rows in tests/am1/raw/a1-<row>-<run>.jsonl.
set -uo pipefail
cd "$(dirname "$0")/../../.."
source ${ROWS_ENV:-tests/m3/rows/env.sh}
ROW=$1 REPO=$2 ID=$3; shift 3
{ date -u +%FT%TZ; git rev-parse --short HEAD; echo "a1.mjs $ROW --repo $REPO --chain repo:$ID $*"
  node tests/am1/rest/a1.mjs "$ROW" --repo "$REPO" --chain "repo:$ID" "$@"
  echo "exit $?"; date -u +%FT%TZ; echo DONE; } > tests/am1/raw/runs/a1-$ROW-$REPO.log 2>&1
