#!/bin/bash
# Cost on graft-m2b since the fanout site day, from /tmp/ar/usage.py (Analytics) at overage prices. With T0_FILE (a
# file holding a Unix time), from then, graft-m2b only, containers included (tests/fanout/m2bcost.py).
#   [T0_FILE=...] bash tests/fanout/cost.sh   -> one JSON line with "usd"
T0=$(cat "${T0_FILE:-$(dirname "$0")/raw/siteday.t0}")
iso(){ date -u -d @$1 +%Y-%m-%dT%H:%M:%SZ; }
D=$(dirname "$0")
# With T0_FILE: graft-m2b's own usage only, containers included (m2bcost.py). Without it, the first fanout round's
# formula, kept so its recorded figures can be reproduced: its DO rows and active time are account-wide.
if [ -n "${T0_FILE:-}" ]; then exec python3 "$D/m2bcost.py" "$(iso $T0)" "$(iso $(date +%s))"; fi
python3 /tmp/ar/usage.py "$(iso $T0)" "$(iso $(date +%s))" | python3 -c '
import json,sys
u=json.loads(sys.stdin.read())
usd=u["workers_requests"]*0.30/1e6+u["workers_cpu_ms"]*0.02/1e6+u["do_requests_m2b"]*0.15/1e6+u["do_rows_written_all"]*1.0/1e6+u["do_active_s_all"]*0.125*12.5/1e6
u["usd"]=round(usd,2); print(json.dumps(u))'
