#!/usr/bin/env python3
"""routewall.py FROM_ISO TO_ISO SERVICE -> a member script's handler wall time by route (count, median, p90, p99, max)
from the observability API's calculations view, Durable Object `Member` events only. Workers Logs samples events, so
counts are a sample; the percentiles are over it. Token from the environment, only as a header."""
import json, os, sys, urllib.request
from datetime import datetime
acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
ms = lambda s: int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
a, b, svc = ms(sys.argv[1]), ms(sys.argv[2]), sys.argv[3]
W = "$workers.wallTimeMs"
calcs = [{"operator": "count"}] + [{"operator": o, "key": W, "keyType": "number"} for o in ("median", "p90", "p99", "max")]
out = {}
for route in ("/commit", "/outcome", "/outcome_by_request", "/raft", "/read", "/suspect"):
    f = [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": svc},
         {"key": "$workers.entrypoint", "operation": "eq", "type": "string", "value": "Member"},
         {"key": "$workers.event.request.url", "operation": "eq", "type": "string", "value": "https://member" + route}]
    body = {"queryId": "q", "timeframe": {"from": a, "to": b}, "view": "calculations",
            "parameters": {"filters": f, "calculations": calcs, "limit": 1}}
    r = urllib.request.Request(f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
                               data=json.dumps(body).encode(), method="POST",
                               headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
    for c in json.load(urllib.request.urlopen(r, timeout=90))["result"]["calculations"]:
        for x in c["aggregates"]:
            out.setdefault(route, {})[c["calculation"].split("(")[0]] = round(x["value"])
for k, v in sorted(out.items(), key=lambda z: -z[1].get("count", 0)):
    print(f"  {k:20} n {v.get('count'):6}  median {v.get('median'):6}  p90 {v.get('p90'):6}  p99 {v.get('p99'):6}  max {v.get('max')}")
