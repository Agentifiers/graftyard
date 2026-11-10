#!/bin/bash
# Cost on graft-m2b since the fanout site day, from /tmp/ar/usage.py (Analytics) at overage prices. With T0_FILE (a
# file holding a Unix time), from then; containers are added as "containers_usd", measured from
# containersUsageAdaptiveGroups for the graft-m2b-site-*-gitshard applications, and included in "usd".
#   [T0_FILE=...] bash tests/fanout/cost.sh   -> one JSON line with "usd"
T0=$(cat "${T0_FILE:-$(dirname "$0")/raw/siteday.t0}")
iso(){ date -u -d @$1 +%Y-%m-%dT%H:%M:%SZ; }
D=$(dirname "$0")
C=$(python3 "$D/containers.py" "$(iso $T0)" "$(iso $(date +%s))")
python3 /tmp/ar/usage.py "$(iso $T0)" "$(iso $(date +%s))" | C="$C" python3 -c '
import json,os,sys
u=json.loads(sys.stdin.read())
usd=u["workers_requests"]*0.30/1e6+u["workers_cpu_ms"]*0.02/1e6+u["do_requests_m2b"]*0.15/1e6+u["do_rows_written_all"]*1.0/1e6+u["do_active_s_all"]*0.125*12.5/1e6
try: c=json.loads(os.environ["C"])
except Exception: c={"containers_usd": None}
u.update(c); u["usd"]=round(usd+(c.get("containers_usd") or 0),2); print(json.dumps(u))'
