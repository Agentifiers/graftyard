#!/usr/bin/env python3
"""BF-46: graft-ops' log lines from Workers Logs in a window, the lines a `Rotate` writes (its probes and key rows, its
`IntentStep`s, and any page) and the run summaries. One line per event: the time and the message, account ids
removed by the caller's scrub. Needs CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_API_TOKEN (sent as a header only).

  logs.py SCRIPT FROM_MS TO_MS     (KEEP=a,b,c: keep lines naming any of these instead; MATCH=s: query lines
                                    containing s instead of "graft-ops", such as LF-7's "graft-agents")
"""
import json, os, sys, urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
script, frm, to = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
URL = f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query"
KEEP = tuple(os.environ["KEEP"].split(",")) if os.environ.get("KEEP") else (
    "Rotate", "PAGE", "IntentStep", "IntentDone", "Committed", "Paged", "Error")


def query(a, b):
    body = {"queryId": "q", "timeframe": {"from": a, "to": b}, "view": "events", "limit": 100,
            "parameters": {"filters": [
                {"key": "$metadata.service", "operation": "eq", "type": "string", "value": script},
                {"key": "$metadata.message", "operation": "includes", "type": "string", "value": os.environ.get("MATCH", "graft-ops")}]}}
    r = urllib.request.Request(URL, data=json.dumps(body).encode(), method="POST",
                               headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
    return json.load(urllib.request.urlopen(r, timeout=60)).get("result", {}).get("events", {}).get("events", [])


seen = set()
for a in range(frm, to, 120_000):
    for e in sorted(query(a, min(to, a + 120_000)), key=lambda e: e["timestamp"]):
        msg = e.get("$metadata", {}).get("message") or ""
        if (e["timestamp"], msg) in seen or not any(k in msg for k in KEEP):
            continue
        seen.add((e["timestamp"], msg))
        print(json.dumps({"at": e["timestamp"], "msg": msg[:400]}))
