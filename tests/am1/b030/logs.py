#!/usr/bin/env python3
"""logs.py SCRIPT FROM_UNIX TO_UNIX [NEEDLE] -> Workers Logs events for SCRIPT (graft-m1-site-a etc.) as JSON lines (a copy of am1-runs' logs.py).
Token from the environment, by header only."""
import json, os, sys, urllib.request
acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
script, a, b = sys.argv[1], int(float(sys.argv[2]) * 1000), int(float(sys.argv[3]) * 1000)
needle = sys.argv[4] if len(sys.argv) > 4 else None
step = int(os.environ.get("STEP", "600")) * 1000
def q(a, b):
    f = [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": script}]
    if needle:
        f.append({"key": "$metadata.message", "operation": "includes", "type": "string", "value": needle})
    body = {"queryId": "q", "timeframe": {"from": a, "to": b}, "view": "events", "limit": 100, "parameters": {"filters": f}}
    r = urllib.request.Request(f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
        data=json.dumps(body).encode(), headers={"authorization": f"Bearer {tok}", "content-type": "application/json"}, method="POST")
    return json.load(urllib.request.urlopen(r, timeout=60)).get("result", {}).get("events", {}).get("events", [])
seen = set()
for t in range(a, b, step):
    for e in q(t, min(b, t + step)):
        m = e.get("$metadata", {})
        k = (e.get("timestamp"), m.get("message"))
        if k in seen: continue
        seen.add(k)
        print(json.dumps({"at": e.get("timestamp"), "level": m.get("level"), "msg": (m.get("message") or "")[:600], "err": m.get("error"), "trigger": m.get("trigger"), "outcome": e.get("$workers", {}).get("outcome"), "cpu": e.get("$workers", {}).get("cpuTimeMs"), "wall": e.get("$workers", {}).get("wallTimeMs"), "entry": e.get("$workers", {}).get("entrypoint")}))
