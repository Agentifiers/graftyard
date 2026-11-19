#!/bin/bash
# A gate row's harness outages (tests/fanout/README.md, "Harness outages on gate rows"), appended to its status:
# the edge and sites' arrivals per second over the load window (arrivals.py, to tests/fanout/raw/arrivals-<tag>.jsonl),
# the outage windows and their share, then every line as measured and with the windows removed (outagecut.py).
#   bash tests/fanout/outages.sh <tag> <row> <repo>   (b029.sh and b033.sh call it after the row)
set -uo pipefail
cd "$(dirname "$0")/../.."
TAG=$1 ROW=$2 REPO=$3 R=tests/fanout/raw
ST=$R/rows-$TAG.status
log=tests/am1/raw/runs/a1-$ROW-$REPO.log
# "b0-29 run <RUN>: 500 agents, 600 s from <ISO>, leader site 0"
line=$(grep -E "^[^ ]+ $ROW run [a-z0-9]+: .* s from " "$log" | tail -1)
run=$(echo "$line" | sed -E "s/.* $ROW run ([a-z0-9]+):.*/\1/")
secs=$(echo "$line" | sed -E 's/.*, ([0-9]+) s from .*/\1/')
from=$(echo "$line" | sed -E 's/.* s from ([0-9T:.-]+Z).*/\1/')
[ -n "$run" ] && [ -n "$secs" ] && [ -n "$from" ] || { echo "outages: no load window in $log" >> "$ST"; exit 0; }
to=$(python3 -c "import datetime,sys; t=datetime.datetime.fromisoformat(sys.argv[1].replace('Z','+00:00'))+datetime.timedelta(seconds=int(sys.argv[2])); print(t.strftime('%Y-%m-%dT%H:%M:%SZ'))" "$from" "$secs")
from=${from%.*}Z
sum=$(python3 tests/fanout/arrivals.py "${RUN_PREFIX:-graft-m2b}" "$from" "$to" $R/arrivals-$TAG.jsonl 2>&1 | tail -1)
{ echo "harness outages ($from to $to, edge 0 while the sites serve; over 5% of the window: invalid, rerun):"
  echo "$sum"
  s=tests/am1/raw/a1-samples-$ROW-$run.jsonl.gz
  # KINDS (space-separated) replaces outagecut.py's default lines, as b034.sh's client and edge clocks.
  if [ -f "$s" ]; then python3 tests/fanout/outagecut.py "$s" "$sum" ${KINDS:-}; else echo "no $s: lines as measured only"; fi
} >> "$ST" 2>&1
