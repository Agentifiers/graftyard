#!/usr/bin/env python3
"""fivexx.py FROM_ISO TO_ISO SERVICE STEP_S -> one service's 5xx per STEP_S by route class and status, from the observability
API's calculations view (the B0-33 m50/m51 5xx attribution, tests/am1/results/c-design.md). Token from the environment."""
import json, os, sys, urllib.request, re
from collections import Counter, defaultdict
from datetime import datetime, timezone
acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
ms = lambda s: int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
a, b, svc, step = ms(sys.argv[1]), ms(sys.argv[2]), sys.argv[3], int(sys.argv[4]) * 1000
def kind(p):
    for k, s in [("upload-pack", "upload-pack"), ("receive-pack", "receive-pack"), ("graphql", "graphql"), ("renew", "/renew"),
                 ("lease", "/leases"), ("task", "/tasks/"), ("label", "/labels"), ("merge", "/merge")]:
        if s in p: return k
    return p if not p.startswith("/api") else "other"
t = a
tot = Counter()
while t < b:
    f = [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": svc},
         {"key": "$workers.event.response.status", "operation": "gte", "type": "number", "value": 500}]
    body = {"queryId": "q", "timeframe": {"from": t, "to": t + step}, "view": "calculations", "parameters": {"filters": f,
        "calculations": [{"operator": "count"}],
        "groupBys": [{"type": "string", "value": "$workers.event.path"}, {"type": "number", "value": "$workers.event.response.status"}], "limit": 1000}}
    r = urllib.request.Request(f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
        data=json.dumps(body).encode(), method="POST", headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
    res = json.load(urllib.request.urlopen(r, timeout=90))["result"]["calculations"]
    c = Counter()
    for x in res[0]["aggregates"]:
        k = f'{kind(x["groups"][0]["value"])} {x["groups"][1]["value"]}'
        c[k] += int(x["count"]); tot[k] += int(x["count"])
    print(datetime.fromtimestamp(t / 1000, timezone.utc).strftime("%H:%M:%S"), dict(c.most_common()))
    t += step
print("total", sum(tot.values()), dict(tot.most_common()))

