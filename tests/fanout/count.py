#!/usr/bin/env python3
"""count.py SCRIPT FROM_ISO TO_ISO GROUPBY [GROUPBY...] -> exact counts of Workers Logs events for SCRIPT in the
window, grouped by the given fields (the observability API's `calculations` view; logs.py's `events` view returns at
most 100 a query). Fields such as `$workers.event.request.url`, `$workers.event.response.status`, `$workers.entrypoint`,
`$metadata.message`. Account and token from the environment, the token only as a header."""
import json, os, re, sys, urllib.request
from datetime import datetime

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
script, a, b, *groups = sys.argv[1:]
ms = lambda s: int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
body = {"queryId": "q", "timeframe": {"from": ms(a), "to": ms(b)}, "view": "calculations", "limit": 200, "parameters": {
    "filters": [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": script}],
    "calculations": [{"operator": "count"}],
    "groupBys": [{"type": "string", "value": g} for g in groups]}}
r = urllib.request.Request(f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
                           data=json.dumps(body).encode(), method="POST",
                           headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
v = json.load(urllib.request.urlopen(r, timeout=60))
out = {}
for x in (v.get("result", {}).get("calculations") or [{}])[0].get("aggregates", []):
    # Ids (4 or more digits) folded, so one route is one line (and no query string reaches the output).
    k = " | ".join(re.sub(r"\d{4,}", "N", str(g.get("value")).split("?")[0]) for g in x.get("groups", []))
    out[k] = out.get(k, 0) + int(x.get("count", 0))
for k, n in sorted(out.items(), key=lambda kv: -kv[1]):
    print(f"{n:8d}  {k}")
