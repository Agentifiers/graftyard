#!/usr/bin/env python3
"""instcpu.py FROM_ISO TO_ISO [NAME_LIKE] -> each graft-m2b shard container instance's CPU per minute (vCPU, from
containersUsageAdaptiveGroups by instanceId), named by the containers API's instance list (`git#<site>#net:<repo>` or
`git#<site>#pool:<n>`), with its start and its end state, and per minute the container's peak memory (GiB) and disk (GB)
from containersMetricsAdaptiveGroups. Account and token from the environment, the token only as a
header. Instances the API no longer lists are shown by id."""
import collections, json, os, sys, urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
a, b = sys.argv[1], sys.argv[2]
like = sys.argv[3] if len(sys.argv) > 3 else ""
P = os.environ.get("INSTALL", "graft-m2b")
H = {"authorization": f"Bearer {tok}", "content-type": "application/json"}
api = lambda p: json.load(urllib.request.urlopen(urllib.request.Request(
    f"https://api.cloudflare.com/client/v4/accounts/{acct}/containers/{p}", headers=H), timeout=60))["result"]
apps = {x["id"].replace("-", ""): x for x in api("applications") if x["name"].startswith(f"{P}-site-")}
inst = {}
for aid, app in apps.items():
    for i in api(f"applications/{app['id']}/instances?per_page=1000")["instances"]:
        inst[i["id"]] = i
q = """query($a: String!, $s: Time!, $e: Time!) { viewer { accounts(filter: {accountTag: $a}) {
  c: containersUsageAdaptiveGroups(limit: 10000, filter: {datetime_geq: $s, datetime_lt: $e}) {
    sum { cpuTimeSec } dimensions { applicationId instanceId datetimeMinute } } } } }"""
v = json.load(urllib.request.urlopen(urllib.request.Request("https://api.cloudflare.com/client/v4/graphql",
    data=json.dumps({"query": q, "variables": {"a": acct, "s": a, "e": b}}).encode(), headers=H), timeout=60))
if v.get("errors"):
    sys.exit(json.dumps(v["errors"])[:800])
qm = """query($a: String!, $s: Time!, $e: Time!) { viewer { accounts(filter: {accountTag: $a}) {
  c: containersMetricsAdaptiveGroups(limit: 10000, filter: {datetime_geq: $s, datetime_lt: $e}) {
    max { memory diskUsage } dimensions { instanceId datetimeMinute } } } } }"""
vm = json.load(urllib.request.urlopen(urllib.request.Request("https://api.cloudflare.com/client/v4/graphql",
    data=json.dumps({"query": qm, "variables": {"a": acct, "s": a, "e": b}}).encode(), headers=H), timeout=60))
mem = collections.defaultdict(dict)
for r in (vm.get("data") or {}).get("viewer", {}).get("accounts", [{}])[0].get("c", []):
    d = r["dimensions"]
    mem[d["instanceId"]][d["datetimeMinute"][11:16]] = [round(r["max"]["memory"] / 2**30, 2), round(r["max"]["diskUsage"] / 1e9, 1)]
t = collections.defaultdict(dict)
ids = {}
for r in v["data"]["viewer"]["accounts"][0]["c"]:
    d = r["dimensions"]
    if d["applicationId"].replace("-", "") not in apps:
        continue
    i = inst.get(d["instanceId"], {})
    name = i.get("name") or d["instanceId"][:12]
    if like and like not in name:
        continue
    t[name][d["datetimeMinute"][11:16]] = round(r["sum"]["cpuTimeSec"] / 60, 2)
    ids[name] = d["instanceId"]
for name in sorted(t):
    i = next((x for x in inst.values() if x.get("name") == name), {})
    print(json.dumps({"name": name, "started": i.get("started_at"), "state": i.get("status", {}).get("state"),
                      "exit": i.get("status", {}).get("exit_code"), "vcpu_by_minute": dict(sorted(t[name].items())),
                      "mem_gib_disk_gb_by_minute": dict(sorted(mem.get(ids.get(name), {}).items()))}))
