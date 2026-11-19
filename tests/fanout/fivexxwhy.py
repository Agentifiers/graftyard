#!/usr/bin/env python3
"""fivexxwhy.py FROM_ISO TO_ISO [SCRIPT] -> the edge's 5xx on a row by reason: its `{"fivexx", "kind", "why"}` log
lines (one per 5xx reply, `fetch` in `workers/edge/src/lib.rs`), counted by (status, kind, reason). Numbers and hex
ids in the reason become N and H, so one cause is one row. Printed against every 5xx the edge answered in the window,
so a gap shows replies the line missed. Workers Logs' exact `calculations` view; the token from the environment, as a
header."""
import collections, json, os, re, sys, urllib.request
from datetime import datetime

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
a, b = sys.argv[1:3]
svc = sys.argv[3] if len(sys.argv) > 3 else "graft-m2b-edge"
ms = lambda s: int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)


def count(filters, groups):
    body = {"queryId": "q", "timeframe": {"from": ms(a), "to": ms(b)}, "view": "calculations", "limit": 2000,
            "parameters": {"filters": [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": svc}]
                           + filters, "calculations": [{"operator": "count"}],
                           "groupBys": [{"type": t, "value": g} for g, t in groups], "limit": 2000}}
    r = urllib.request.Request(f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
                               data=json.dumps(body).encode(), method="POST",
                               headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
    agg = json.load(urllib.request.urlopen(r, timeout=90))["result"]["calculations"][0]["aggregates"]
    return [([str(g["value"]) for g in x["groups"]], int(x["count"])) for x in agg]


def norm(why):
    return re.sub(r"\d+", "N", re.sub(r"[0-9a-f]{16,}", "H", why))


# Workers Logs parses a JSON line into fields (`fivexx`, `kind`, `why`) and keeps no `$metadata.message` for it, so the
# lines are counted by those fields.
by = collections.Counter()
for (s, k, w), n in count([{"key": "fivexx", "operation": "exists", "type": "number"}],
                          [("fivexx", "number"), ("kind", "string"), ("why", "string")]):
    by[(s, k, norm(w))] += n
status = {g[0]: n for g, n in count([{"key": "$workers.event.response.status", "operation": "gte", "type": "number",
                                      "value": 500}], [("$workers.event.response.status", "number")])}
logged, answered = sum(by.values()), sum(status.values())
print(json.dumps({"logged": logged, "edge5xx": answered, "byStatus": status}))
kinds = collections.Counter()
for (s, k, _), n in by.items():
    kinds[(s, k)] += n
for (s, k), n in kinds.most_common():
    print(f"{n:7d} {s} {k}")
print("by reason:")
for (s, k, w), n in by.most_common(40):
    print(f"{n:7d} {s} {k:16s} {w}")
