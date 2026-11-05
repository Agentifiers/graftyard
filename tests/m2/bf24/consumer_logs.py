#!/usr/bin/env python3
"""The PrSync consumer's log lines on the three sites from Workers Logs: consumer_logs.py FROM_UNIX TO_UNIX > out.jsonl.
One JSON line per handled message: site, time, batch, queue delay, handler time, and the handler's steps (record, read,
merges, commits, in ms) or `skipped`. Needs CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_API_TOKEN."""
import json, os, re, sys, urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
frm, to = int(float(sys.argv[1]) * 1000), int(float(sys.argv[2]) * 1000)
# The install's script prefix, as bf24.py reads it.
PREFIX = os.environ.get("BF24_PREFIX", "graft-m1")
# A queue delivery's line, or a first attempt's (`first handled`, no queue delay).
LINE = re.compile(r"prsync (\d+) (\d+)\.\.(\d+) (?:queued (\d+)ms|first) handled (\d+)ms (.*)")


def query(script, a, b):
    body = {"queryId": "q", "timeframe": {"from": a, "to": b}, "view": "events", "limit": 100,
            "parameters": {"filters": [
                {"key": "$metadata.service", "operation": "eq", "type": "string", "value": script},
                {"key": "$metadata.message", "operation": "includes", "type": "string", "value": "prsync "}]}}
    r = urllib.request.Request(
        f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
        data=json.dumps(body).encode(), headers={"authorization": f"Bearer {tok}", "content-type": "application/json"},
        method="POST")
    return json.load(urllib.request.urlopen(r, timeout=60)).get("result", {}).get("events", {}).get("events", [])


for site in "abc":
    seen = set()
    # 10-minute windows keep each query under the 100-event cap.
    for a in range(frm, to, 600_000):
        for e in query(f"{PREFIX}-site-{site}", a, min(to, a + 600_000)):
            msg = e.get("$metadata", {}).get("message") or ""
            key = (e["timestamp"], msg)
            if key in seen:
                continue
            seen.add(key)
            m = LINE.search(msg)
            if not m:
                print(json.dumps({"site": site, "at": e["timestamp"], "error": msg[-300:]}))
                continue
            v = json.loads(m.group(6))
            print(json.dumps({"site": site, "at": e["timestamp"], "repo": int(m.group(1)), "from": int(m.group(2)),
                              "through": int(m.group(3)), "path": "queue" if m.group(4) else "first",
                              "queued_ms": int(m.group(4) or 0), "handled_ms": int(m.group(5)),
                              "steps_ms": v.get("ms"), "skipped": v.get("skipped")}))
