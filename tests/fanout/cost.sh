#!/bin/bash
# Cost on graft-m2b since the fanout site day, from /tmp/ar/usage.py (Analytics) at overage prices; containers excluded.
#   bash tests/fanout/cost.sh   -> one JSON line with "usd"
T0=$(cat "$(dirname "$0")/raw/siteday.t0")
iso(){ date -u -d @$1 +%Y-%m-%dT%H:%M:%SZ; }
python3 /tmp/ar/usage.py "$(iso $T0)" "$(iso $(date +%s))" | python3 -c '
import json,sys
u=json.loads(sys.stdin.read())
usd=u["workers_requests"]*0.30/1e6+u["workers_cpu_ms"]*0.02/1e6+u["do_requests_m2b"]*0.15/1e6+u["do_rows_written_all"]*1.0/1e6+u["do_active_s_all"]*0.125*12.5/1e6
u["usd"]=round(usd,2); print(json.dumps(u))'
