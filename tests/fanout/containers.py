#!/usr/bin/env python3
"""containers.py FROM_ISO TO_ISO -> the graft-m2b-site-*-gitshard containers' billed usage in the window, from
containersUsageAdaptiveGroups (the dashboard's billing numbers; memory and disk in byte-seconds), priced at
research/prices.json's cf.containers.* overage rates (included allowance ignored). Account and token from the
environment, the token sent only as a header."""
import json, os, sys, urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
a, b = sys.argv[1], sys.argv[2]
# The install by its script prefix: graft-m2b unless INSTALL names another, such as graft-m3c.
P = os.environ.get("INSTALL", "graft-m2b")
H = {"authorization": f"Bearer {tok}", "content-type": "application/json"}
apps = json.load(urllib.request.urlopen(urllib.request.Request(
    f"https://api.cloudflare.com/client/v4/accounts/{acct}/containers/applications", headers=H), timeout=60))["result"]
ids = {x["id"].replace("-", ""): x["name"] for x in apps if x["name"].startswith(f"{P}-site-")}
q = """query($a: String!, $s: Time!, $e: Time!) { viewer { accounts(filter: {accountTag: $a}) {
  c: containersUsageAdaptiveGroups(limit: 100, filter: {datetime_geq: $s, datetime_lt: $e}) {
    sum { cpuTimeSec allocatedMemory allocatedDisk } dimensions { applicationId } } } } }"""
v = json.load(urllib.request.urlopen(urllib.request.Request("https://api.cloudflare.com/client/v4/graphql",
    data=json.dumps({"query": q, "variables": {"a": acct, "s": a, "e": b}}).encode(), headers=H), timeout=60))
if v.get("errors"):
    print(json.dumps(v["errors"])[:800]); sys.exit(1)
rows = [r for r in v["data"]["viewer"]["accounts"][0]["c"] if r["dimensions"]["applicationId"].replace("-", "") in ids]
cpu = sum(r["sum"]["cpuTimeSec"] for r in rows)
mem = sum(r["sum"]["allocatedMemory"] for r in rows) / 2**30
disk = sum(r["sum"]["allocatedDisk"] for r in rows) / 1e9
usd = cpu * 0.000020 + mem * 0.0000025 + disk * 0.00000007
print(json.dumps({"containers_cpu_s": round(cpu), "containers_gib_s": round(mem), "containers_gb_s": round(disk),
                  "containers_usd": round(usd, 2)}))
