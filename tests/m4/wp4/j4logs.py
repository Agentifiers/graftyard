#!/usr/bin/env python3
"""j4logs.py SCRIPT FROM_ISO TO_ISO NEEDLE [NEEDLE...] -> per minute, how many Workers Logs events of SCRIPT carry every
NEEDLE in their message, plus the first and last such event's time and message (cut at 220 characters). For the j4
(repo:65536) leaderless window after site day 54. Account and token from the environment, the token only as a header."""
import json, os, sys, urllib.request
from datetime import datetime, timezone

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
script, a, b, *needles = sys.argv[1:]
ms = lambda s: int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
iso = lambda t: datetime.fromtimestamp(t / 1000, timezone.utc).strftime("%H:%M:%S.%f")[:-3] + "Z"
filters = [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": script}]
filters += [{"key": "$metadata.message", "operation": "includes", "type": "string", "value": n} for n in needles]


def query(view, extra):
    body = {"queryId": "q", "timeframe": {"from": ms(a), "to": ms(b)}, "view": view, "limit": 100,
            "parameters": {"filters": filters, **extra}}
    r = urllib.request.Request(
        f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
        data=json.dumps(body).encode(), method="POST",
        headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
    return json.load(urllib.request.urlopen(r, timeout=60)).get("result", {})


calc = query("calculations", {"calculations": [{"operator": "count"}], "granularity": 60})
series = (calc.get("calculations") or [{}])[0].get("series") or []
total = 0
for s in series:
    for p in s.get("data", []):
        n = int(p.get("count") or 0)
        if n:
            total += n
            print(f"{p.get('time')}  {n}")
print("total", total)
for order in ("asc", "desc"):
    ev = query("events", {"orderBy": {"value": "$metadata.timestamp", "order": order}}).get("events", {}).get("events", [])
    if ev:
        e = ev[0]
        print(order, iso(e.get("timestamp", 0)), str(e.get("$metadata", {}).get("message", ""))[:220])
