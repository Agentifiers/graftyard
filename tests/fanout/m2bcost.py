#!/usr/bin/env python3
"""m2bcost.py FROM_ISO TO_ISO -> graft-m2b's own usage and cost in the window, as one JSON line: Workers requests and
CPU of the graft-m2b-* scripts, Durable Object requests, rows written and active time of the graft-m2b-* namespaces
only (durableObjectsPeriodicGroups by namespace; the account's other installs, such as graft-m1 running B0-30, are
left out), and the graft-m2b-site-*-gitshard containers (containers.py). Priced at research/prices.json's overage
rates, included allowances ignored. Account and token from the environment, the token only as a header."""
import json, os, subprocess, sys, urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
a, b = sys.argv[1], sys.argv[2]
H = {"authorization": f"Bearer {tok}", "content-type": "application/json"}
ns = json.load(urllib.request.urlopen(urllib.request.Request(
    f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/durable_objects/namespaces?per_page=1000",
    headers=H), timeout=60))["result"]
mine = {n["id"] for n in ns if (n.get("script") or "").startswith("graft-m2b")}
q = """query($a: String!, $s: Time!, $e: Time!) { viewer { accounts(filter: {accountTag: $a}) {
  w: workersInvocationsAdaptive(limit: 100, filter: {datetime_geq: $s, datetime_lt: $e, scriptName_like: "graft-m2b%"}) {
    sum { requests cpuTimeUs } }
  d: durableObjectsInvocationsAdaptiveGroups(limit: 100, filter: {datetime_geq: $s, datetime_lt: $e}) {
    sum { requests } dimensions { scriptName } }
  p: durableObjectsPeriodicGroups(limit: 1000, filter: {datetime_geq: $s, datetime_lt: $e}) {
    sum { rowsWritten activeTime } dimensions { namespaceId } } } } }"""
v = json.load(urllib.request.urlopen(urllib.request.Request("https://api.cloudflare.com/client/v4/graphql",
    data=json.dumps({"query": q, "variables": {"a": acct, "s": a, "e": b}}).encode(), headers=H), timeout=60))
if v.get("errors"):
    print(json.dumps(v["errors"])[:800]); sys.exit(1)
acc = v["data"]["viewer"]["accounts"][0]
u = {
    "workers_requests": sum(x["sum"]["requests"] for x in acc["w"]),
    "workers_cpu_ms": round(sum(x["sum"]["cpuTimeUs"] for x in acc["w"]) / 1000),
    "do_requests": sum(x["sum"]["requests"] for x in acc["d"] if (x["dimensions"]["scriptName"] or "").startswith("graft-m2b")),
    "do_rows_written": sum(x["sum"]["rowsWritten"] for x in acc["p"] if x["dimensions"]["namespaceId"] in mine),
    "do_active_s": round(sum(x["sum"]["activeTime"] for x in acc["p"] if x["dimensions"]["namespaceId"] in mine) / 1e6),
}
usd = (u["workers_requests"] * 0.30 / 1e6 + u["workers_cpu_ms"] * 0.02 / 1e6 + u["do_requests"] * 0.15 / 1e6
       + u["do_rows_written"] * 1.0 / 1e6 + u["do_active_s"] * 0.125 * 12.5 / 1e6)
c = json.loads(subprocess.run([sys.executable, os.path.join(os.path.dirname(__file__), "containers.py"), a, b],
                              capture_output=True, text=True, check=True).stdout)
u.update(c)
u["usd"] = round(usd + c["containers_usd"], 2)
print(json.dumps(u))
