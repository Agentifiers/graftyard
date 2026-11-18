#!/usr/bin/env python3
"""fivexxwall.py FROM_ISO TO_ISO [SERVICE] -> one service's 5xx events with wall time, by status and route class, in
STEP-second pages of the events view (the B0-33 m52 shed-backoff check against m51's 18,294 × 5xx at p50 22 s).
Prints n, p50, p99 and max wall per (class, status), and writes the rows to FIVEXX_OUT if set. Token from the
environment, only as a header."""
import collections, json, os, sys, urllib.request
from datetime import datetime, timezone
acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
ms = lambda s: int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
a, b = ms(sys.argv[1]), ms(sys.argv[2])
svc = sys.argv[3] if len(sys.argv) > 3 else "graft-m2b-edge"
STEP = int(os.environ.get("STEP", "5")) * 1000
pct = lambda xs, q: (sorted(xs)[min(len(xs) - 1, int(q * len(xs)))] if xs else None)


def kind(p):
    for k, s in [("upload-pack", "upload-pack"), ("receive-pack", "receive-pack"), ("graphql", "graphql"),
                 ("renew", "/renew"), ("lease", "/leases"), ("task", "/tasks/"), ("label", "/labels"), ("merge", "/merge")]:
        if s in p:
            return k
    return "other"


rows, full = [], 0
t = a
while t < b:
    f = [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": svc},
         {"key": "$workers.event.response.status", "operation": "gte", "type": "number", "value": 500}]
    body = {"queryId": "q", "timeframe": {"from": t, "to": t + STEP}, "view": "events", "limit": 2000,
            "parameters": {"filters": f}}
    r = urllib.request.Request(f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
                               data=json.dumps(body).encode(), method="POST",
                               headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
    for i in range(4):
        try:
            ev = json.load(urllib.request.urlopen(r, timeout=90))["result"].get("events", {}).get("events", [])
            break
        except Exception:
            if i == 3:
                raise
    full += len(ev) >= 2000
    for e in ev:
        w = e["$workers"]
        if w.get("eventType") not in (None, "fetch") or "response" not in w.get("event", {}):
            continue
        rows.append((e["timestamp"], kind(w["event"]["request"].get("path") or w["event"]["request"].get("url", "")), w["event"]["response"]["status"],
                     w.get("wallTimeMs")))
    t += STEP
g = collections.defaultdict(list)
for x in rows:
    g[(x[1], x[2])].append(x[3] or 0)
allw = [x[3] or 0 for x in rows]
print(f"{svc} {sys.argv[1]}..{sys.argv[2]}: {len(rows)} × 5xx, wall p50 {pct(allw, .5)} p99 {pct(allw, .99)} ms"
      + (f" ({full} pages hit the 2,000 limit: lower STEP)" if full else ""))
for k, v in sorted(g.items(), key=lambda z: -len(z[1])):
    print(f"  {k[0]:13} {k[1]}  n {len(v):6}  wall p50 {pct(v, .5)}  p99 {pct(v, .99)}  max {max(v)}")
by = collections.Counter((x[0] // 30000) * 30 for x in rows)
print("per 30 s:", " ".join(f"{datetime.fromtimestamp(k, timezone.utc).strftime('%H:%M:%S')}={v}" for k, v in sorted(by.items())))
if os.environ.get("FIVEXX_OUT"):
    json.dump(rows, open(os.environ["FIVEXX_OUT"], "w"))
