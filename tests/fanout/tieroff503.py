#!/usr/bin/env python3
"""tieroff503.py FROM_ISO TO_ISO [SCRIPT] -> `tier-off-leader`'s 503s on a row: the edge's `{"shed": "tierOff"}` log
lines (`api::read_failed`), by the read that failed (`why`), against every 503 the edge answered in the window. The
B0-33 half of the flip fires only if these are a large share of the row's 503s (`decisions/DECISIONS.md`,
"`tier-off-leader`"). A leaderless group's shed after its probe, `{"shed": "elect"}`, is counted apart (`elect`): an
edge built before F128 logged it as `tierOff`, so on such an edge `tierOff` holds both. Workers Logs' exact
`calculations` view; the token from the environment, as a header."""
import json, os, sys, urllib.request
from datetime import datetime

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
a, b = sys.argv[1:3]
svc = sys.argv[3] if len(sys.argv) > 3 else "graft-m2b-edge"
ms = lambda s: int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)


def count(filters, group):
    body = {"queryId": "q", "timeframe": {"from": ms(a), "to": ms(b)}, "view": "calculations", "parameters": {
        "filters": [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": svc}] + filters,
        "calculations": [{"operator": "count"}], "groupBys": [{"type": "string", "value": group}]}}
    r = urllib.request.Request(f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
                               data=json.dumps(body).encode(), method="POST",
                               headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
    agg = json.load(urllib.request.urlopen(r, timeout=60))["result"]["calculations"][0]["aggregates"]
    return {str(x["groups"][0]["value"]): int(x["count"]) for x in agg}


def sheds(name):
    lines = count([{"key": "$metadata.message", "operation": "includes", "type": "string", "value": f'"shed":"{name}"'}],
                  "$metadata.message")
    by_why = {}
    for msg, n in lines.items():
        try:
            why = json.loads(msg).get("why", "?")
        except ValueError:
            why = "?"
        by_why[why] = by_why.get(why, 0) + n
    return by_why


tier, elect = sheds("tierOff"), sheds("elect")
fivexx = count([{"key": "$workers.event.response.status", "operation": "eq", "type": "number", "value": 503}],
               "$workers.event.response.status")
n_tier, n_elect, n_503 = sum(tier.values()), sum(elect.values()), sum(fivexx.values())
print(json.dumps({"tierOff": n_tier, "elect": n_elect, "edge503": n_503,
                  "share": round(n_tier / n_503, 4) if n_503 else None,
                  "electShare": round(n_elect / n_503, 4) if n_503 else None,
                  "byWhy": dict(sorted(tier.items(), key=lambda kv: -kv[1])),
                  "electByWhy": dict(sorted(elect.items(), key=lambda kv: -kv[1]))}))
