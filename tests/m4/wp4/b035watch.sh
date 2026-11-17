#!/bin/bash
# B0-35's stop watcher on graft-m4a (slot 2026-11-17): every 2 minutes, acctcost by PREFIX=graft-m4a plus the runner
# account since FROM ($8 stop), the install account's UTC day ($280 stop), and any Sandbox instance not `inactive` in
# the runner account's applications (stop: the workflow's label has no runner). The fourth stop, a run for an entry
# over 1 h old, is checked from the run: the repository is new, so every entry it holds is newer than FROM. On a stop
# it writes tests/m4/raw/wp4/b035-$TAG.STOP; the thread then stops the service.
#   FROM=<iso> TAG=<tag> bash tests/m4/wp4/b035watch.sh
set -uo pipefail
cd "$(dirname "$0")/../../.."
source tests/am1/lf13/env-m4a.sh >/dev/null 2>&1
FROM=${FROM:?} TAG=${TAG:?}
OUT=tests/m4/raw/wp4/b035-$TAG.watch
STOP=tests/m4/raw/wp4/b035-$TAG.STOP
RUNNER=(env CLOUDFLARE_ACCOUNT_ID=<cf-runner-account-id> CLOUDFLARE_API_TOKEN="$CLOUDFLARE_RUNNER_API_TOKEN")
usd() { python3 -c "import json,sys;print(json.loads(sys.stdin.readline()).get('$1',0))"; }
while true; do
  N=$(date -u +%FT%TZ)
  M=$(PREFIX=graft-m4a python3 tests/fanout/acctcost.py "$FROM" "$N" 2>/dev/null | usd prefix_usd)
  DAY=$(python3 tests/fanout/acctcost.py "${N:0:10}T00:00:00Z" "$N" 2>/dev/null | usd usd)
  RUN=$("${RUNNER[@]}" python3 tests/fanout/acctcost.py "$FROM" "$N" 2>/dev/null | usd usd)
  SB=$("${RUNNER[@]}" python3 - <<'PY' 2>/dev/null || echo "?"
import json, os, urllib.request
a, t = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
get = lambda p: json.load(urllib.request.urlopen(urllib.request.Request(
    f"https://api.cloudflare.com/client/v4/accounts/{a}/{p}", headers={"authorization": f"Bearer {t}"}), timeout=60))["result"]
n = 0
for app in get("containers/applications"):
    for i in get(f"containers/applications/{app['id']}/instances?per_page=1000")["instances"]:
        n += (i.get("status") or {}).get("state", "inactive") != "inactive"
print(n)
PY
)
  T=$(python3 -c "print(round(${M:-0}+${RUN:-0},2))")
  echo "$N m4a $M runner $RUN mine $T account-day $DAY sandboxes-running $SB" >> "$OUT"
  if python3 -c "import sys;sys.exit(0 if ${T}>=8 or ${DAY:-0}>=280 else 1)"; then echo "$N STOP cost mine $T day $DAY" > "$STOP"; fi
  if [ "$SB" != "?" ] && [ "$SB" -gt 0 ]; then echo "$N STOP sandboxes running $SB" > "$STOP"; fi
  sleep 120
done
