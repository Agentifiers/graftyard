#!/usr/bin/env python3
"""commitsenders.py OBJ FROM_ISO TO_ISO [OUT] -> who sent each `/commit` a Member object received, from Workers Logs.

Reads every `/commit` event at Durable Object OBJ (the leader's id) in FROM..TO from the events view, in slices of at
most 4 s (halved while a page comes back full at 500), and splits them by `x-graft-sent-by` (`SENT_BY` in
crates/lookup/src/lib.rs, isolate × 16 + step; step 1 `Leader` is `commit_as`/`call_opt`'s post to the cached leader,
4 `Send` is `commit_hedged`'s receipt send). A `/commit` with no header came from a script built without test-faults
(graft-m4a's edge). Prints per minute: edge / top isolate / other isolates, the steps seen, and the wall-time share of
1 s or more per sender; then totals. Raw rows to OUT as JSON lines when given. Account and token from the environment.
"""
import collections, json, os, sys, time, urllib.request
from datetime import datetime, timezone

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
ms = lambda s: int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
obj, F, T = sys.argv[1], ms(sys.argv[2]), ms(sys.argv[3])
out = open(sys.argv[4], "w") if len(sys.argv) > 4 else None


def q(a, b):
    body = {"queryId": "q", "timeframe": {"from": a, "to": b}, "view": "events", "limit": 500,
            "parameters": {"filters": [
                {"key": "$workers.durableObjectId", "operation": "eq", "type": "string", "value": obj},
                {"key": "$workers.event.path", "operation": "eq", "type": "string", "value": "/commit"}]}}
    err = None
    for i in range(4):
        try:
            r = urllib.request.Request(
                f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
                data=json.dumps(body).encode(), method="POST",
                headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
            return json.load(urllib.request.urlopen(r, timeout=60))["result"]["events"]["events"]
        except Exception as e:
            err = e
            time.sleep(3 * (i + 1))
    raise err


rows, seen, lo, saturated = [], set(), F, 0
while lo < T:
    hi = min(T, lo + 4000)
    while True:
        got = q(lo, hi)
        if len(got) < 500 or hi - lo <= 250:
            break
        hi = lo + (hi - lo) // 2
    saturated += len(got) >= 500
    for e in got:
        m, w = e["$metadata"], e.get("$workers", {})
        if m["id"] in seen:
            continue
        seen.add(m["id"])
        by = w.get("event", {}).get("request", {}).get("headers", {}).get("x-graft-sent-by")
        r = {"t": e["timestamp"], "wall": w.get("wallTimeMs"), "by": by}
        rows.append(r)
        if out:
            out.write(json.dumps(r) + "\n")
    lo = hi
iso = collections.Counter(int(r["by"]) // 16 for r in rows if r["by"])
top = iso.most_common(1)[0][0] if iso else None
kind = lambda r: "edge" if not r["by"] else ("top" if int(r["by"]) // 16 == top else "other")
per = collections.defaultdict(collections.Counter)
for r in rows:
    per[datetime.fromtimestamp(r["t"] / 1000, timezone.utc).strftime("%H:%M")][kind(r)] += 1
print(f"{len(rows)} /commit at {obj[:12]}; saturated slices {saturated}; top isolate {top}")
print("minute edge/top/other: " + " ".join(f"{m} {c['edge']}/{c['top']}/{c['other']}" for m, c in sorted(per.items())))
steps = collections.Counter(("edge" if not r["by"] else int(r["by"]) % 16) for r in rows)
print("steps (1 Leader, 4 Send, edge no header):", dict(steps))
for k in ("edge", "top", "other"):
    w = [r["wall"] for r in rows if kind(r) == k and r["wall"] is not None]
    if w:
        w.sort()
        print(f"{k}: {len(w)} calls, wall p50 {w[len(w) // 2]} ms p90 {w[int(len(w) * .9)]} ms, "
              f">=1 s {100 * sum(x >= 1000 for x in w) / len(w):.0f}%")
sec = collections.Counter(r["t"] // 1000 for r in rows)
print("peak a second:", max(sec.values()) if sec else 0)
