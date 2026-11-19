#!/usr/bin/env python3
"""mergewl.py SERVICE FROM TO -> each REST merge PUT the edge served in [FROM, TO), one JSON line per merge, from the
Workers Logs events view: {"ts": the event's timestamp (the request's end), "wall": its wall ms, "cpu", "status",
"colo"}. The request's start is ts - wall; `tierahead.py` pairs these with a1's `mergeSteps`. Account and token from
the environment (CLOUDFLARE_ACCOUNT_ID, CLOUDFLARE_API_TOKEN), the token only as a header."""
import json
import os
import sys
import urllib.request
from datetime import datetime


def ms(s):
    return int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)


def main():
    service, a, b = sys.argv[1:4]
    acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
    f = lambda k, op, v: {"key": k, "operation": op, "type": "string", "value": v}
    body = {"queryId": "q", "timeframe": {"from": ms(a), "to": ms(b)}, "view": "events", "limit": 500,
            "parameters": {"filters": [f("$metadata.service", "eq", service), f("$metadata.url", "includes", "/merge")]}}
    r = urllib.request.Request(f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
                               data=json.dumps(body).encode(), method="POST",
                               headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
    events = json.load(urllib.request.urlopen(r, timeout=55))["result"].get("events", {}).get("events", [])
    for e in sorted(events, key=lambda e: e["timestamp"]):
        w = e["$workers"]
        ev = w.get("event", {})
        print(json.dumps({"ts": e["timestamp"], "wall": w.get("wallTimeMs"), "cpu": w.get("cpuTimeMs"),
                          "status": ev.get("response", {}).get("status"), "colo": ev.get("request", {}).get("cf", {}).get("colo")}))


if __name__ == "__main__":
    main()
