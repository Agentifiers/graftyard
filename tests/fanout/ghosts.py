#!/usr/bin/env python3
"""ghosts.py FROM_ISO TO_ISO -> every graft-m2b gitshard container instance billed in the window
(containersUsageAdaptiveGroups by instanceId), with its name and state from the Containers API instance list, or
UNLISTED. The list can miss a running instance (git#2#net:131210 on November 11, tests/am1/results/idle-shards.md),
which teardown.py then never sees; its instance id is the shard object id, so Workers Logs' durableObjectId names
its network. Account and token from the environment, the token only as a header."""
import json, os, sys, urllib.request
acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
H = {"authorization": f"Bearer {tok}", "content-type": "application/json"}
g = lambda u: json.load(urllib.request.urlopen(urllib.request.Request("https://api.cloudflare.com/client/v4/accounts/"+acct+u, headers=H), timeout=60))["result"]
apps = {a["id"]: a["name"] for a in g("/containers/applications") if a["name"].startswith("graft-m2b-site-")}
q = """query($a: String!, $s: Time!, $e: Time!) { viewer { accounts(filter: {accountTag: $a}) {
  c: containersUsageAdaptiveGroups(limit: 5000, filter: {datetime_geq: $s, datetime_lt: $e}) {
    sum { allocatedMemory cpuTimeSec } dimensions { applicationId instanceId } } } } }"""
v = json.load(urllib.request.urlopen(urllib.request.Request("https://api.cloudflare.com/client/v4/graphql",
    data=json.dumps({"query": q, "variables": {"a": acct, "s": sys.argv[1], "e": sys.argv[2]}}).encode(), headers=H), timeout=60))
rows = v["data"]["viewer"]["accounts"][0]["c"]
for aid, name in apps.items():
    listed = {i["id"]: i for i in g(f"/containers/applications/{aid}/instances?per_page=1000")["instances"]}
    for r in rows:
        if r["dimensions"]["applicationId"].replace("-", "") != aid: continue
        iid = r["dimensions"]["instanceId"]; i = listed.get(iid)
        print(name[-10:], iid[:12], (i or {}).get("name", "UNLISTED"), (i or {}).get("status", {}).get("state", "-"),
              round(r["sum"]["allocatedMemory"] / 2**30), "GiB-s", round(r["sum"]["cpuTimeSec"], 1), "cpu-s")
