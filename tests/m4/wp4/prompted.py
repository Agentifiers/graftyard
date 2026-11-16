#!/usr/bin/env python3
"""prompted.py FROM_ISO TO_ISO -> which trigger started `graft-runners`' passes in the window (R24's hosted step).

Reads the runner account's Workers Logs for `graft-runners`: every invocation with its time, its trigger (`fetch` for an
App delivery, `scheduled` for the 5-minute sweep), the response status a `fetch` answered, and each pass's
`graft-runners pass: started N ...` line, joined by request id. Prints one JSON line per invocation, then a summary:
the passes that started runners, by trigger. Account and token from the environment (`CLOUDFLARE_ACCOUNT_ID`,
`CLOUDFLARE_API_TOKEN`), the token only as a header.
"""
import json, os, re, sys, urllib.request
from datetime import datetime, timezone

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
a, b = sys.argv[1], sys.argv[2]
ms = lambda s: int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
iso = lambda t: datetime.fromtimestamp(t / 1000, timezone.utc).strftime("%H:%M:%S")


def query(lo, hi, offset=None):
    body = {"queryId": "q", "timeframe": {"from": lo, "to": hi}, "view": "events", "limit": 100, "parameters": {
        "filters": [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": "graft-runners"}]}}
    if offset:
        body["offset"] = offset
    r = urllib.request.Request(
        f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
        data=json.dumps(body).encode(), method="POST",
        headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
    return json.load(urllib.request.urlopen(r, timeout=60))["result"]["events"]["events"]


# The events view answers at most 100 a query, newest first: walk back through the window in slices.
events, hi, lo = [], ms(b), ms(a)
while hi > lo:
    got = query(lo, hi)
    events += got
    if len(got) < 100:
        break
    hi = min(e["timestamp"] for e in got)
seen, by_req = set(), {}
for e in events:
    m = e["$metadata"]
    if m["id"] in seen:
        continue
    seen.add(m["id"])
    w = e.get("$workers", {})
    r = by_req.setdefault(m.get("requestId"), {"t": e["timestamp"], "trigger": w.get("eventType"), "lines": []})
    r["t"] = min(r["t"], e["timestamp"])
    ev = w.get("event") or {}
    if w.get("eventType") == "fetch":
        r["status"] = (ev.get("response") or {}).get("status", r.get("status"))
        r["path"] = (ev.get("request") or {}).get("url", "").split("?")[0]
    msg = m.get("message") or ""
    if msg.startswith("graft-runners"):
        r["lines"].append(msg[:200])
summary = {}
for rid, r in sorted(by_req.items(), key=lambda kv: kv[1]["t"]):
    started = sum(int(x) for l in r["lines"] for x in re.findall(r"pass: started (\d+)", l))
    r["started"] = started
    print(json.dumps({"at": iso(r["t"]), **{k: v for k, v in r.items() if k != "t"}}))
    s = summary.setdefault(r["trigger"] or "?", {"invocations": 0, "passes_that_started": 0, "runners_started": 0})
    s["invocations"] += 1
    s["passes_that_started"] += started > 0
    s["runners_started"] += started
print(json.dumps({"summary": summary}))
