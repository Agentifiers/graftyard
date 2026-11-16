#!/usr/bin/env python3
"""near.py FROM_ISO TO_ISO -> B0-30 rerun (operator, 2026-11-16): per hour, the edge's git-receive-pack answers by
status, and how many 503s fell within NEAR_S (3) of a StepDown on the group's own members versus outside every one.
The edge does not log which 503 it sent (`shard_unavailable`, no leader, pack store), so a 503 near a hold is only
consistent with a commit-side cause [inference]. The events view caps a query at 100, so it reads 20 s windows.
Token from the environment, by header only."""
import collections, datetime as D, json, os, sys, time, urllib.error, urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
URL = f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query"
ms = lambda iso: int(D.datetime.fromisoformat(iso.replace("Z", "+00:00")).timestamp() * 1000)
NEAR = int(os.environ.get("NEAR_S", "3")) * 1000
ids = {l.strip() for l in open("tests/am1/raw/b030-objects.txt") if l.strip()}


def events(svc, needle, fr, to, step):
    out = []
    for t in range(fr, to, step):
        body = {"queryId": "q", "timeframe": {"from": t, "to": min(to, t + step)}, "view": "events", "limit": 100,
                "parameters": {"filters": [
                    {"key": "$metadata.service", "operation": "eq", "type": "string", "value": svc},
                    {"key": "$metadata.message", "operation": "includes", "type": "string", "value": needle}]}}
        req = urllib.request.Request(URL, data=json.dumps(body).encode(), method="POST",
                                     headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
        for i in range(3):
            try:
                ev = json.load(urllib.request.urlopen(req, timeout=60))["result"]["events"]["events"]
                break
            except (urllib.error.URLError, TimeoutError):
                if i == 2:
                    raise
                time.sleep(5 * (i + 1))
        if len(ev) >= 100 and step > 2000:
            out += events(svc, needle, t, min(to, t + step), step // 4)
        else:
            out += ev
    return out


a, b = ms(sys.argv[1]), ms(sys.argv[2])
holds = sorted(e["timestamp"] for s in "abc" for e in events(f"graft-m1-site-{s}", "StepDown", a, b, 600_000)
               if e.get("$workers", {}).get("durableObjectId") in ids)
hours = collections.defaultdict(lambda: {"ok": 0, "503": 0, "other": 0, "503near": 0, "503outside": 0})
seen = set()
for e in events("graft-m1-edge", "b0-30-r2.git/git-receive-pack", a, b, 20_000):
    k = (e["timestamp"], e.get("$workers", {}).get("requestId"))
    if k in seen:
        continue
    seen.add(k)
    st = ((e.get("$workers", {}).get("event") or {}).get("response") or {}).get("status")
    h = hours[D.datetime.fromtimestamp(e["timestamp"] / 1000, D.UTC).strftime("%H")]
    if st == 200:
        h["ok"] += 1
    elif st == 503:
        h["503"] += 1
        end = e["timestamp"]
        start = end - (e.get("$workers", {}).get("wallTimeMs") or 0)
        near = any(start - NEAR <= t <= end + NEAR for t in holds)
        h["503near" if near else "503outside"] += 1
    else:
        h["other"] += 1
for hr, v in sorted(hours.items()):
    print(json.dumps({"hour": hr + "Z", **v, "holds": sum(1 for t in holds if D.datetime.fromtimestamp(t / 1000, D.UTC).strftime("%H") == hr)}))
