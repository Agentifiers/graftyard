#!/bin/bash
# Stops a row at once if the edge logs lookup-share's failure lines (m42, 2026-11-17: "Cannot perform I/O on behalf of
# a different request", "closure invoked recursively or after being dropped", "Cannot invoke closure from previous WASM
# instance"), then redeploys the edge from BACK_REV's worktree and logs it. Polls Workers Logs every 30 s until the
# row's done marker appears.
#   TAG=<row tag> DONE_FILE=<file> DONE_MARK=B033DONE BACK_DIR=<worktree at the fallback rev> bash tests/fanout/edgeguard.sh
#   -> tests/fanout/raw/edgeguard-<tag>.status
set -uo pipefail
cd "$(dirname "$0")/../.."
source tests/m3/rows/env.sh
ST=tests/fanout/raw/edgeguard-$TAG.status
s() { echo "$(date -u +%FT%TZ) $*" >> "$ST"; }
s "watching graft-m2b-edge for lookup-share's error lines"
count() {
  python3 - "$1" <<'EOF'
import json, os, sys, time, urllib.request
acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
now = int(time.time() * 1000); n = 0
for needle in ("Cannot perform I/O on behalf of a different request", "closure invoked recursively",
               "Cannot invoke closure from previous WASM instance"):
    body = {"queryId": "q", "timeframe": {"from": now - int(sys.argv[1]) * 1000, "to": now}, "view": "events", "limit": 100,
            "parameters": {"filters": [
                {"key": "$metadata.service", "operation": "eq", "type": "string", "value": "graft-m2b-edge"},
                {"key": "$metadata.message", "operation": "includes", "type": "string", "value": needle}]}}
    r = urllib.request.Request(f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
        data=json.dumps(body).encode(), headers={"authorization": f"Bearer {tok}", "content-type": "application/json"}, method="POST")
    n += len(json.load(urllib.request.urlopen(r, timeout=60)).get("result", {}).get("events", {}).get("events", []))
print(n)
EOF
}
until grep -q "$DONE_MARK\$" "$DONE_FILE" 2>/dev/null; do
  sleep 30
  n=$(count 60 2>/dev/null || echo "?")
  s "lookup-share error lines in the last 60 s: $n"
  if [ "$n" != "?" ] && [ "$n" -gt 0 ]; then
    s "STOP: edge errors back; stopping a1.mjs and redeploying the edge from $BACK_DIR"
    pkill -f "a1.mjs b0-33 " ; s "a1 stopped"
    (cd "$BACK_DIR" && OPS=graft-m2b-ops bash tests/am1/runs/deploy-edge.sh "fanout-$TAG-back-edge.log" 32.8,32.12,32.14,32.18,18.3,32.25) >> "$ST" 2>&1
    s "edge redeployed: $(python3 tests/fanout/edgever.py "$BACK_DIR/tests/am1/raw/runs/fanout-$TAG-back-edge.log" 2>&1)"
    break
  fi
done
s "guard done"
echo DONE >> "$ST"
exec sleep infinity
