#!/usr/bin/env python3
"""promomem.py FROM_ISO TO_ISO [SCRIPT_LIKE] -> each graft-m2b site script's maximum memory in the window, by
status and version: `workersInvocationsAdaptive` max `memoryUsageBytes` and `wasmMemoryBytes`, as stream-push.md read
the edge's, by minute. A streamed promotion runs as its own site-script invocation (`/archive/promote`, `archive::in_band`), so
its isolate's memory shows here. Account and token from the environment, the token only as a header."""
import json, os, sys, urllib.request

a, b = sys.argv[1], sys.argv[2]
like = sys.argv[3] if len(sys.argv) > 3 else "graft-m2b-site-%"
q = ('{ viewer { accounts(filter:{accountTag:"%s"}) { workersInvocationsAdaptive(limit:500, orderBy:[datetimeMinute_ASC], '
     'filter:{scriptName_like:"%s", datetime_geq:"%s", datetime_leq:"%s"}) { dimensions { scriptName status scriptVersion datetimeMinute } '
     'sum { requests } max { memoryUsageBytes wasmMemoryBytes } } } } }') % (os.environ["CLOUDFLARE_ACCOUNT_ID"], like, a, b)
r = urllib.request.Request("https://api.cloudflare.com/client/v4/graphql", data=json.dumps({"query": q}).encode(),
                           headers={"authorization": f"Bearer {os.environ['CLOUDFLARE_API_TOKEN']}",
                                    "content-type": "application/json"})
v = json.load(urllib.request.urlopen(r, timeout=60))
if v.get("errors"):
    print(json.dumps({"error": json.dumps(v["errors"])[:400]}))
    sys.exit(1)
for x in v["data"]["viewer"]["accounts"][0]["workersInvocationsAdaptive"]:
    d = x["dimensions"]
    print(json.dumps({"minute": d["datetimeMinute"][11:16], "script": d["scriptName"], "version": d["scriptVersion"][:8], "status": d["status"],
                      "requests": x["sum"]["requests"], "max_mb": round(x["max"]["memoryUsageBytes"] / 1e6, 1),
                      "max_wasm_mb": round(x["max"]["wasmMemoryBytes"] / 1e6, 1)}))
