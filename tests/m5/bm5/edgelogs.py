"""BM-5 clause 3 (`mint.sh checks`): the edge's MintWatch lines from Workers Logs between two times (ms).

Usage: python3 tests/m5/bm5/edgelogs.py <script> <from ms> <to ms>
Prints a count per needle, then up to 10 lines of each, oldest first.
"""
import json
import os
import sys
import time
import urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
script, frm, to = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
URL = f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query"
for needle in ("mint undo", "MINT_ERROR_RATIO", "graft-ops did not mint"):
    body = {"queryId": "q", "timeframe": {"from": frm, "to": to}, "view": "events", "limit": 100,
            "parameters": {"filters": [
                {"key": "$metadata.service", "operation": "eq", "type": "string", "value": script},
                {"key": "$metadata.message", "operation": "includes", "type": "string", "value": needle}]}}
    req = urllib.request.Request(URL, data=json.dumps(body).encode(), method="POST",
                                 headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
    try:
        ev = json.load(urllib.request.urlopen(req, timeout=60)).get("result", {}).get("events", {}).get("events", [])
    except Exception as e:  # noqa: BLE001 - a failed query is reported, not fatal
        print(json.dumps({"needle": needle, "error": str(e)}))
        time.sleep(10)
        continue
    print(json.dumps({"needle": needle, "lines": len(ev)}))
    for e in sorted(ev, key=lambda e: e["timestamp"])[:10]:
        md = e.get("$metadata", {})
        print(json.dumps({"at": time.strftime("%H:%M:%S", time.gmtime(e["timestamp"] / 1000)),
                          "isolate": (e.get("$workers", {}) or {}).get("scriptVersion", {}).get("id"),
                          "msg": (md.get("message") or "")[:300]}))
    time.sleep(3)
