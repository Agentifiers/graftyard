#!/usr/bin/env python3
"""m2bcost.py FROM_ISO TO_ISO -> one install's own usage and cost in the window, as one JSON line: Workers requests and
CPU of its scripts, Durable Object requests, rows written and active time of its namespaces only
(durableObjectsPeriodicGroups by namespace; the account's other installs are left out), and its site gitshard
containers (containers.py). The install is graft-m2b unless INSTALL names another (`env-m3f.sh` sets graft-m3f); the
line ends with `install` and the account's name. Priced at research/prices.json's overage rates, included allowances
ignored. Account and token from the environment, the token only as a header."""
import json, os, subprocess, sys, urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
a, b = sys.argv[1], sys.argv[2]
# The install by its script prefix: graft-m2b unless INSTALL names another, such as graft-m3c.
P = os.environ.get("INSTALL", "graft-m2b")
H = {"authorization": f"Bearer {tok}", "content-type": "application/json"}
ns = json.load(urllib.request.urlopen(urllib.request.Request(
    f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/durable_objects/namespaces?per_page=1000",
    headers=H), timeout=60))["result"]
mine = {n["id"] for n in ns if (n.get("script") or "").startswith(P)}
q = """query($a: String!, $s: Time!, $e: Time!, $p: String!) { viewer { accounts(filter: {accountTag: $a}) {
  w: workersInvocationsAdaptive(limit: 100, filter: {datetime_geq: $s, datetime_lt: $e, scriptName_like: $p}) {
    sum { requests cpuTimeUs } }
  d: durableObjectsInvocationsAdaptiveGroups(limit: 100, filter: {datetime_geq: $s, datetime_lt: $e}) {
    sum { requests } dimensions { scriptName } }
  p: durableObjectsPeriodicGroups(limit: 1000, filter: {datetime_geq: $s, datetime_lt: $e}) {
    sum { rowsWritten activeTime } dimensions { namespaceId } } } } }"""
v = json.load(urllib.request.urlopen(urllib.request.Request("https://api.cloudflare.com/client/v4/graphql",
    data=json.dumps({"query": q, "variables": {"a": acct, "s": a, "e": b, "p": P + "%"}}).encode(), headers=H), timeout=60))
if v.get("errors"):
    print(json.dumps(v["errors"])[:800]); sys.exit(1)
acc = v["data"]["viewer"]["accounts"][0]
u = {
    "workers_requests": sum(x["sum"]["requests"] for x in acc["w"]),
    "workers_cpu_ms": round(sum(x["sum"]["cpuTimeUs"] for x in acc["w"]) / 1000),
    "do_requests": sum(x["sum"]["requests"] for x in acc["d"] if (x["dimensions"]["scriptName"] or "").startswith(P)),
    "do_rows_written": sum(x["sum"]["rowsWritten"] for x in acc["p"] if x["dimensions"]["namespaceId"] in mine),
    "do_active_s": round(sum(x["sum"]["activeTime"] for x in acc["p"] if x["dimensions"]["namespaceId"] in mine) / 1e6),
}
usd = (u["workers_requests"] * 0.30 / 1e6 + u["workers_cpu_ms"] * 0.02 / 1e6 + u["do_requests"] * 0.15 / 1e6
       + u["do_rows_written"] * 1.0 / 1e6 + u["do_active_s"] * 0.125 * 12.5 / 1e6)
c = json.loads(subprocess.run([sys.executable, os.path.join(os.path.dirname(__file__), "containers.py"), a, b],
                              capture_output=True, text=True, check=True).stdout)
u.update(c)
u["usd"] = round(usd + c["containers_usd"], 2)
# Which install and which account the reading is for, printed beside it: run without `env-m3f.sh` sourced, this meter
# priced graft-m2b while the readings were taken as graft-m3f's (`tests/am1/results/lm1-rows-m3f.md`, Cost).
name = json.load(urllib.request.urlopen(urllib.request.Request(
    f"https://api.cloudflare.com/client/v4/accounts/{acct}", headers=H), timeout=60))["result"]["name"]
u["install"], u["account"] = P, name
print(json.dumps(u))
