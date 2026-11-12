#!/usr/bin/env python3
"""usage.py FROM_ISO TO_ISO -> graft-m3c's usage in the window and its cost at overage rates, as one JSON line.

tests/am1/b030/usage.py for graft-m3c: Workers requests and CPU and Durable Object requests of graft-m3c's three
sites and its edge, and Durable Object rows written and active time on the sites' namespaces, found by script name.
Containers are priced from `CONTAINER_S` (instance-seconds of standard-2, passed by the caller from the monitor's
shard rows) at research/prices.json's rates: 1 vCPU, 6 GiB, 12 GB. The token goes in a header only."""
import json, os, sys, urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
a, b = sys.argv[1], sys.argv[2]
SCRIPTS = ["graft-m3c-site-a", "graft-m3c-site-b", "graft-m3c-site-c", "graft-m3c-edge"]
H = {"authorization": f"Bearer {tok}", "content-type": "application/json"}


def get(path):
    r = urllib.request.Request(f"https://api.cloudflare.com/client/v4/accounts/{acct}/{path}", headers=H)
    return json.load(urllib.request.urlopen(r, timeout=60))["result"]


ns = {n["id"] for n in get("workers/durable_objects/namespaces?per_page=1000") if n.get("script") in SCRIPTS}
q = """query($a: String!, $s: Time!, $e: Time!) { viewer { accounts(filter: {accountTag: $a}) {
  w: workersInvocationsAdaptive(limit: 100, filter: {datetime_geq: $s, datetime_lt: $e, scriptName_like: "graft-m3c-%"}) { sum { requests cpuTimeUs } dimensions { scriptName } }
  d: durableObjectsInvocationsAdaptiveGroups(limit: 100, filter: {datetime_geq: $s, datetime_lt: $e}) { sum { requests } dimensions { scriptName } }
  p: durableObjectsPeriodicGroups(limit: 1000, filter: {datetime_geq: $s, datetime_lt: $e}) { sum { rowsWritten activeTime } dimensions { namespaceId } }
} } }"""
r = urllib.request.Request("https://api.cloudflare.com/client/v4/graphql",
    data=json.dumps({"query": q, "variables": {"a": acct, "s": a, "e": b}}).encode(), headers=H)
v = json.load(urllib.request.urlopen(r, timeout=60))
if v.get("errors"):
    print(json.dumps({"error": json.dumps(v["errors"])[:400]}))
    sys.exit(1)
acc = v["data"]["viewer"]["accounts"][0]
w = [x for x in acc["w"] if x["dimensions"]["scriptName"] in SCRIPTS]
wreq = sum(x["sum"]["requests"] for x in w)
cpu_ms = sum(x["sum"]["cpuTimeUs"] for x in w) / 1000
dreq = sum(x["sum"]["requests"] for x in acc["d"] if x["dimensions"]["scriptName"] in SCRIPTS)
p = [x for x in acc["p"] if x["dimensions"]["namespaceId"] in ns]
rows = sum(x["sum"]["rowsWritten"] for x in p)
act_s = sum(x["sum"]["activeTime"] for x in p) / 1e6
cs = float(os.environ.get("CONTAINER_S", "0"))
# standard-2: 1 vCPU at $0.000020/vCPU-s, 6 GiB at $0.0000025/GiB-s, 12 GB disk at $0.00000007/GB-s.
containers = cs * (1 * 0.000020 + 6 * 0.0000025 + 12 * 0.00000007)
cost = wreq * 0.30e-6 + cpu_ms * 0.02e-6 + dreq * 0.15e-6 + rows * 1.0e-6 + act_s * 0.128 * 12.5e-6 + containers
print(json.dumps({"from": a, "to": b, "workersRequests": wreq, "workersCpuMs": round(cpu_ms), "doRequests": dreq,
    "doRowsWritten": rows, "doActiveS": round(act_s), "containerS": cs, "containers": round(containers, 2),
    "cost": round(cost, 2)}))
