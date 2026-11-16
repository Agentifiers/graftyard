#!/usr/bin/env python3
"""usage.py FROM_ISO TO_ISO -> graft-m1's usage in the window and its cost at overage rates, as one JSON line.

From Cloudflare's GraphQL Analytics (the same query as am1-runs' usage.py, filtered to graft-m1's sites a to d and
its edge): Workers requests and CPU, Durable Object requests, and Durable Object rows written and active time on the
four sites' namespaces. R2 Class A puts and storage are estimated by the caller from the archive counts, and
Containers are not reported here.

With B030_OBJECTS (a file of Durable Object ids, one a line; tests/am1/b030/objects.py writes it), Durable Object
requests, rows written and active time count only those objects, so a run is not charged for another group on the
same install (B0-30 rerun: smoke 1's `repo:1048586` reinstalling on site b, 19.5 M rows). Workers requests and CPU
have no object dimension and stay install-wide; `workersInstallWide` says so. Prices from research/prices.json's overage rates, as am1-runs priced. The token
goes in a header only."""
import json, os, sys, urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
a, b = sys.argv[1], sys.argv[2]
SCRIPTS = ["graft-m1-site-a", "graft-m1-site-b", "graft-m1-site-c", "graft-m1-site-d", "graft-m1-edge"]
# graft-m1-site-a..d's Member, Registry, and GitShard namespaces.
NS = {
    "815ce539fb69415eb7e76603bcf96841", "f38407dc896b4e779fd1aa83aeb161b7", "0c84d783cbf649848efa493a8ed52dc3",
    "7769d6f6ddec41aaa22be88c4ab2123f", "81aac5a0baff48a39ded22f2838c9d82", "335538d9a6294e05831e5de8ae1d0332",
    "e55c3c34bbf94f7dbb7cc96f107cf2d0", "4a39cf08f86746fc8c019e8dfe8eaf56", "6e63e22e7ab84e808176104f970acfd1",
    "aee636012e9d4a42bc8a4e1d0cd7bba9", "a4e13cb19ca84cd98d3977c7bb69d064", "2b9348e989ff4510a1766850af190c75",
}
q = """query($a: String!, $s: Time!, $e: Time!) { viewer { accounts(filter: {accountTag: $a}) {
  w: workersInvocationsAdaptive(limit: 100, filter: {datetime_geq: $s, datetime_lt: $e, scriptName_like: "graft-m1-%"}) { sum { requests cpuTimeUs } dimensions { scriptName } }
  d: durableObjectsInvocationsAdaptiveGroups(limit: 100, filter: {datetime_geq: $s, datetime_lt: $e@OBJ@}) { sum { requests } dimensions { scriptName } }
  p: durableObjectsPeriodicGroups(limit: 1000, filter: {datetime_geq: $s, datetime_lt: $e@OBJ@}) { sum { rowsWritten rowsRead activeTime } dimensions { namespaceId } }
} } }"""
objects = [l.strip() for l in open(os.environ["B030_OBJECTS"]) if l.strip()] if os.environ.get("B030_OBJECTS") else None
q = q.replace("$e: Time!", "$e: Time!, $o: [String!]" if objects else "$e: Time!").replace("@OBJ@", ", objectId_in: $o" if objects else "")
r = urllib.request.Request("https://api.cloudflare.com/client/v4/graphql",
    data=json.dumps({"query": q, "variables": {"a": acct, "s": a, "e": b, **({"o": objects} if objects else {})}}).encode(),
    headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
v = json.load(urllib.request.urlopen(r, timeout=60))
if v.get("errors"):
    print(json.dumps({"error": json.dumps(v["errors"])[:400]}))
    sys.exit(1)
acc = v["data"]["viewer"]["accounts"][0]
w = [x for x in acc["w"] if x["dimensions"]["scriptName"] in SCRIPTS]
wreq = sum(x["sum"]["requests"] for x in w)
cpu_ms = sum(x["sum"]["cpuTimeUs"] for x in w) / 1000
dreq = sum(x["sum"]["requests"] for x in acc["d"] if x["dimensions"]["scriptName"] in SCRIPTS)
p = [x for x in acc["p"] if x["dimensions"]["namespaceId"] in NS]
rows = sum(x["sum"]["rowsWritten"] for x in p)
act_s = sum(x["sum"]["activeTime"] for x in p) / 1e6
cost = wreq * 0.30e-6 + cpu_ms * 0.02e-6 + dreq * 0.15e-6 + rows * 1.0e-6 + act_s * 0.128 * 12.5e-6
print(json.dumps({"from": a, "to": b, "workersRequests": wreq, "workersCpuMs": round(cpu_ms), "doRequests": dreq,
    "doRowsWritten": rows, "doActiveS": round(act_s), "cost": round(cost, 2),
    **({"objects": len(objects), "workersInstallWide": True} if objects else {})}))
