#!/usr/bin/env python3
"""byobject.py FROM_ISO TO_ISO PATH [SCRIPT...] -> per Durable Object (by name), how many Workers Logs events hit
member PATH (e.g. /read, /suspect) in the window on the graft-m2b site scripts, from the observability API's exact
`calculations` view, names from durableObjectsPeriodicGroups. Token from the environment, as a header only."""
import json, os, sys, urllib.request
from datetime import datetime
acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
H = {"authorization": f"Bearer {tok}", "content-type": "application/json"}
a, b, path, *scripts = sys.argv[1:]
scripts = scripts or ["graft-m2b-site-a", "graft-m2b-site-b", "graft-m2b-site-c"]
ms = lambda s: int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
counts = {}
for sc in scripts:
    body = {"queryId": "q", "timeframe": {"from": ms(a), "to": ms(b)}, "view": "calculations", "limit": 200, "parameters": {
        "filters": [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": sc},
                    {"key": "$workers.event.request.url", "operation": "eq", "type": "string", "value": f"https://member{path}"}],
        "calculations": [{"operator": "count"}], "groupBys": [{"type": "string", "value": "$workers.durableObjectId"}]}}
    v = json.load(urllib.request.urlopen(urllib.request.Request(
        f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
        data=json.dumps(body).encode(), method="POST", headers=H), timeout=60))
    for x in v["result"]["calculations"][0]["aggregates"]:
        counts[x["groups"][0]["value"]] = counts.get(x["groups"][0]["value"], 0) + int(x["count"])
q = """query($a:String!,$s:Time!,$e:Time!,$ids:[string!]){viewer{accounts(filter:{accountTag:$a}){
 p: durableObjectsPeriodicGroups(limit: 1000, filter:{datetime_geq:$s, datetime_lt:$e, objectId_in:$ids}) { dimensions { name objectId } } }}}"""
v = json.load(urllib.request.urlopen(urllib.request.Request("https://api.cloudflare.com/client/v4/graphql", data=json.dumps(
    {"query": q, "variables": {"a": acct, "s": a, "e": b, "ids": list(counts)}}).encode(), headers=H), timeout=60))
names = {r["dimensions"]["objectId"]: r["dimensions"]["name"] for r in v["data"]["viewer"]["accounts"][0]["p"]}
fam = {}
for i, n in counts.items():
    nm = names.get(i, "?")
    f = nm.split(":")[0] if ":" in nm else nm
    fam[f] = fam.get(f, 0) + n
print("by family:", json.dumps(dict(sorted(fam.items(), key=lambda kv: -kv[1]))))
for i, n in sorted(counts.items(), key=lambda kv: -kv[1])[:12]:
    print(f"{n:8d}  {names.get(i, i[:12])}")
