#!/bin/bash
# Stops a row at the edge's first 500 or exception (the held-grant row's stop line, 2026-11-19): every 30 s, the main
# edge's invocations in the last 60 s with response status 500, or with an outcome other than `ok` and `canceled` (an
# uncaught exception, exceeded CPU or memory; `canceled` is a client that hung up) from Workers Logs. On any, a1.mjs is stopped and the status says why. Polls until the row's
# done marker appears.
#   TAG=<row tag> SVC=graft-m3f-edge DONE_FILE=<status file> DONE_MARK=B029DONE bash tests/fanout/edge500guard.sh
#   -> tests/fanout/raw/edge500guard-<tag>.status
set -uo pipefail
cd "$(dirname "$0")/../.."
ST=tests/fanout/raw/edge500guard-$TAG.status
s() { echo "$(date -u +%FT%TZ) $*" >> "$ST"; }
s "watching $SVC for status 500 or an exception outcome"
count() {
  python3 - "$SVC" <<'EOF'
import json, os, sys, time, urllib.request
acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
now = int(time.time() * 1000); out = []
for f in ({"key": "$workers.event.response.status", "operation": "eq", "type": "number", "value": 500},
          [{"key": "$workers.outcome", "operation": "neq", "type": "string", "value": "ok"},
           {"key": "$workers.outcome", "operation": "neq", "type": "string", "value": "canceled"}]):
    f = f if isinstance(f, list) else [f]
    body = {"queryId": "q", "timeframe": {"from": now - 60_000, "to": now}, "view": "calculations",
            "parameters": {"filters": [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": sys.argv[1]}] + f,
                           "calculations": [{"operator": "count"}], "limit": 10}}
    r = urllib.request.Request(f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
        data=json.dumps(body).encode(), headers={"authorization": f"Bearer {tok}", "content-type": "application/json"}, method="POST")
    c = json.load(urllib.request.urlopen(r, timeout=60))["result"]["calculations"]
    out.append(int(sum(a.get("value", 0) for x in c for a in x.get("aggregates", []))))
print(f"{out[0]} {out[1]}")
EOF
}
until grep -q "$DONE_MARK" "$DONE_FILE" 2>/dev/null; do
  sleep 30
  n=$(count 2>/dev/null || echo "? ?")
  s "last 60 s: status 500 ${n% *}, exception outcome ${n#* }"
  if [ "${n% *}" != "?" ] && { [ "${n% *}" -gt 0 ] || [ "${n#* }" -gt 0 ]; }; then
    s "STOP: edge 500 or exception; stopping a1.mjs"
    pkill -f "a1.mjs b0-29 " ; s "a1 stopped"
    break
  fi
done
s "done"
