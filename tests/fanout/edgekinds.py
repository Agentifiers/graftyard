#!/usr/bin/env python3
"""edgekinds.py FROM_ISO TO_ISO -> the graft-m2b edge's requests in the window by kind (git info/refs,
receive-pack, upload-pack, GraphQL, REST v3, the graftyard v1 API), each with its status counts, from the
observability API's exact `calculations` view filtered by path substring. Token from the environment, as a header."""
import json, os, sys, urllib.request
from datetime import datetime
acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
a, b = sys.argv[1:3]
ms = lambda s: int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
def q(sub):
    body = {"queryId": "q", "timeframe": {"from": ms(a), "to": ms(b)}, "view": "calculations", "parameters": {
        "filters": [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": "graft-m2b-edge"},
                    {"key": "$workers.event.path", "operation": "includes", "type": "string", "value": sub}],
        "calculations": [{"operator": "count"}], "groupBys": [{"type": "string", "value": "$workers.event.response.status"}]}}
    r = urllib.request.Request(f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
        data=json.dumps(body).encode(), method="POST", headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
    agg = json.load(urllib.request.urlopen(r, timeout=60))["result"]["calculations"][0]["aggregates"]
    return {str(x["groups"][0]["value"]): int(x["count"]) for x in agg}
out = {}
for k, sub in [("info/refs", "/info/refs"), ("receive-pack", "git-receive-pack"), ("upload-pack", "git-upload-pack"),
               ("graphql", "/api/graphql"), ("rest v3", "/api/v3/"), ("graftyard v1", "/api/graftyard/v1/")]:
    c = q(sub)
    out[k] = {"n": sum(c.values()), **dict(sorted(c.items(), key=lambda kv: -kv[1]))}
print(json.dumps(out))
