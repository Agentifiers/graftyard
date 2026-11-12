#!/usr/bin/env python3
"""logs.py SCRIPT FROM_ISO TO_ISO ENTRYPOINT [NEEDLE] -> Workers Logs events of one entrypoint (GitShard, Member, ...)
of SCRIPT (graft-m3c-site-a etc.), as JSON lines with the time, message, outcome and wall time. The telemetry query
answers at most 100 events, so the window is read in STEP seconds (default 60). Needs tests/am1/lf13/env-m3c.sh
sourced; the token goes in a header only."""
import datetime, json, os, sys, urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
ms = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
script, a, b, entry = sys.argv[1], ms(sys.argv[2]), ms(sys.argv[3]), sys.argv[4]
needle = sys.argv[5] if len(sys.argv) > 5 else None
step = int(os.environ.get("STEP", "60")) * 1000
seen = set()
for t in range(a, b, step):
    f = [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": script},
         {"key": "$workers.entrypoint", "operation": "eq", "type": "string", "value": entry}]
    if needle:
        f.append({"key": "$metadata.message", "operation": "includes", "type": "string", "value": needle})
    body = {"queryId": "q", "timeframe": {"from": t, "to": min(b, t + step)}, "view": "events", "limit": 100,
            "parameters": {"filters": f}}
    r = urllib.request.Request(f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
        data=json.dumps(body).encode(), headers={"authorization": f"Bearer {tok}", "content-type": "application/json"}, method="POST")
    for e in json.load(urllib.request.urlopen(r, timeout=60)).get("result", {}).get("events", {}).get("events", []):
        m, w = e.get("$metadata", {}), e.get("$workers", {})
        k = (e.get("timestamp"), m.get("message"))
        if k in seen:
            continue
        seen.add(k)
        print(json.dumps({"at": datetime.datetime.fromtimestamp(e["timestamp"] / 1000, datetime.UTC).strftime("%H:%M:%S"),
                          "msg": (m.get("message") or "")[:1000], "outcome": w.get("outcome"), "wall": w.get("wallTimeMs"),
                          "cpu": w.get("cpuTimeMs"), "err": m.get("error")}))
