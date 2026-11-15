#!/usr/bin/env python3
"""shardtimeline.py FROM_ISO TO_ISO REPO -> per site and minute, the shard object's requests for network REPO from
Workers Logs (`GitShard` fetch events: path, status or outcome, wall time), how many of each route ran at once on
average (summed wall seconds over 60), and the container instance's vCPU in that minute (instcpu.py's source). An
event's timestamp is its request's start, so a request is counted in every minute its [start, start + wall] spans.
Prints one JSON line per site and minute; raw events go to RAW (env) as JSON lines when set."""
import collections, datetime, json, os, sys, urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
H = {"authorization": f"Bearer {tok}", "content-type": "application/json"}
ms = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
A, B, repo = ms(sys.argv[1]), ms(sys.argv[2]), sys.argv[3]
raw = open(os.environ["RAW"], "w") if os.environ.get("RAW") else None
busy = collections.defaultdict(lambda: collections.defaultdict(float))
n = collections.defaultdict(collections.Counter)
for site in "abc":
    a = A - 180_000
    while a < B:
        e2 = min(B, a + 30_000)
        f = [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": f"graft-m2b-site-{site}"},
             {"key": "$metadata.message", "operation": "includes", "type": "string", "value": f"repo={repo}"},
             {"key": "$workers.entrypoint", "operation": "eq", "type": "string", "value": "GitShard"}]
        body = {"queryId": "q", "timeframe": {"from": a, "to": e2}, "view": "events", "limit": 2000,
                "parameters": {"filters": f}}
        r = urllib.request.Request(
            f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
            data=json.dumps(body).encode(), headers=H, method="POST")
        for attempt in range(4):
            try:
                ev = json.load(urllib.request.urlopen(r, timeout=90)).get("result", {}).get("events", {}).get("events", [])
                break
            except Exception as x:
                print(f"site {site} {a}: {x}, retry", file=sys.stderr)
        else:
            sys.exit(f"site {site} {a}: the query failed 4 times")
        if len(ev) >= 2000:
            print(f"site {site} {a}: 2000 events, truncated", file=sys.stderr)
        for e in ev:
            w = e.get("$workers", {})
            if w.get("entrypoint") != "GitShard":
                continue
            ev_ = w.get("event", {})
            path = ev_.get("path") or ""
            st = (ev_.get("response") or {}).get("status") or w.get("outcome")
            wall = (w.get("wallTimeMs") or 0) / 1000
            t = e["timestamp"] / 1000
            if raw:
                raw.write(json.dumps({"site": site, "t": t, "path": path, "status": st, "wall": wall}) + "\n")
            n[site][(int(t // 60) * 60, path, str(st))] += 1
            s = t
            while s < t + wall:
                m = int(s // 60) * 60
                busy[site][(m, path)] += min(t + wall, m + 60) - s
                s = m + 60
        a = e2
for site in "abc":
    mins = sorted({m for (m, _) in busy[site]} | {m for (m, _, _) in n[site]})
    for m in mins:
        if m * 1000 < A or m * 1000 >= B:
            continue
        print(json.dumps({"site": site, "minute": datetime.datetime.fromtimestamp(m, datetime.UTC).strftime("%H:%M"),
                          "at_once": {p: round(v / 60, 1) for (mm, p), v in sorted(busy[site].items()) if mm == m},
                          "started": {f"{p} {st}": c for (mm, p, st), c in sorted(n[site].items()) if mm == m}}))
