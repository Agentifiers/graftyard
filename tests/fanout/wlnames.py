#!/usr/bin/env python3
"""wlnames.py FROM_ISO TO_ISO SUBSTR [SUBSTR...] -> per Durable Object name and minute, how many Workers Logs events
on $INSTALL's site scripts (default graft-m3f) have a message that includes each SUBSTR, such as "snapshot_commit",
"install {" or "memory limit". Names from durableObjectsPeriodicGroups. Token from the environment, as a header only."""
import json, os, sys, urllib.request
from datetime import datetime
acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
H = {"authorization": f"Bearer {tok}", "content-type": "application/json"}
a, b, *subs = sys.argv[1:]
ms = lambda s: int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
out = []  # (sub, svc, do, minute, n)
for sub in subs:
    for svc in [f"{os.environ.get('INSTALL', 'graft-m3f')}-site-{x}" for x in "abc"]:
        body = {"queryId": "q", "timeframe": {"from": ms(a), "to": ms(b)}, "view": "calculations", "limit": 500, "granularity": 60,
                "parameters": {"filters": [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": svc},
                                           {"key": "$metadata.message", "operation": "includes", "type": "string", "value": sub}],
                               "calculations": [{"operator": "count"}], "groupBys": [{"type": "string", "value": "$workers.durableObjectId"}]}}
        v = json.load(urllib.request.urlopen(urllib.request.Request(
            f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
            data=json.dumps(body).encode(), method="POST", headers=H), timeout=90))
        c = v["result"]["calculations"][0]
        for s in c.get("series", []):
            for d in s.get("data", []):
                n = int(d.get("count") or 0)
                if n:
                    out.append((sub, svc[-1], d["groups"][0]["value"], s["time"][11:16], n))
ids = sorted({o[2] for o in out})
names = {}
if ids:
    q = """query($a:String!,$s:Time!,$e:Time!,$ids:[string!]){viewer{accounts(filter:{accountTag:$a}){
     p: durableObjectsPeriodicGroups(limit: 1000, filter:{datetime_geq:$s, datetime_lt:$e, objectId_in:$ids}) { dimensions { name objectId } } }}}"""
    v = json.load(urllib.request.urlopen(urllib.request.Request("https://api.cloudflare.com/client/v4/graphql", data=json.dumps(
        {"query": q, "variables": {"a": acct, "s": a, "e": b, "ids": ids}}).encode(), headers=H), timeout=60))
    names = {r["dimensions"]["objectId"]: r["dimensions"]["name"] for r in v["data"]["viewer"]["accounts"][0]["p"]}
agg = {}
for sub, site, do, minute, n in out:
    k = (sub, site, names.get(do, do[:8]), minute)
    agg[k] = agg.get(k, 0) + n
for k in sorted(agg, key=lambda k: (k[0], k[3], k[1])):
    print(f"{k[0][:18]:18s} site {k[1]} {k[2]:24s} {k[3]} {agg[k]:6d}")
