#!/usr/bin/env python3
"""acctcost.py [FROM_ISO [TO_ISO]] -> the whole account's usage cost in the window, every install and every product
the bill charges for, as one JSON line, then the five costliest scripts or applications. Default window: today, UTC.

Unlike m2bcost.py, which prices one install from its own start, this reads every namespace, script, and container
application in the account, because per-install meters missed most of a $2,328 bill (2026-11-16): graft-m1's 30-day
runs and the other installs were on no meter. Priced at the bill's own overage rates (container memory, vCPU, and
disk; Durable Object rows written and read, duration, and requests; Workers requests and CPU), allowances ignored, so
it reads slightly high. With PREFIX set (graft-m2b), the line also carries "prefix_usd": the scripts and container
applications whose names start with it, for a run that shares the account with another install's run. Account and token
from the environment, the token only as a header."""
import collections, datetime, json, os, sys, urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
H = {"authorization": f"Bearer {tok}", "content-type": "application/json"}
now = datetime.datetime.now(datetime.UTC).replace(microsecond=0)
a = sys.argv[1] if len(sys.argv) > 1 else now.strftime("%Y-%m-%dT00:00:00Z")
b = sys.argv[2] if len(sys.argv) > 2 else now.strftime("%Y-%m-%dT%H:%M:%SZ")


def get(path):
    url = f"https://api.cloudflare.com/client/v4/accounts/{acct}/{path}"
    return json.load(urllib.request.urlopen(urllib.request.Request(url, headers=H), timeout=60))["result"]


ns = {n["id"]: n.get("script") or "?" for n in get("workers/durable_objects/namespaces?per_page=1000")}
apps = {x["id"].replace("-", ""): x["name"] for x in get("containers/applications")}
q = """query($a: String!, $s: Time!, $e: Time!) { viewer { accounts(filter: {accountTag: $a}) {
  c: containersUsageAdaptiveGroups(limit: 10000, filter: {datetime_geq: $s, datetime_lt: $e}) {
    sum { cpuTimeSec allocatedMemory allocatedDisk } dimensions { applicationId } }
  p: durableObjectsPeriodicGroups(limit: 10000, filter: {datetime_geq: $s, datetime_lt: $e}) {
    sum { rowsWritten rowsRead activeTime } dimensions { namespaceId } }
  d: durableObjectsInvocationsAdaptiveGroups(limit: 10000, filter: {datetime_geq: $s, datetime_lt: $e}) {
    sum { requests } dimensions { scriptName } }
  w: workersInvocationsAdaptive(limit: 10000, filter: {datetime_geq: $s, datetime_lt: $e}) {
    sum { requests cpuTimeUs } dimensions { scriptName } } } } }"""
v = json.load(urllib.request.urlopen(urllib.request.Request("https://api.cloudflare.com/client/v4/graphql",
    data=json.dumps({"query": q, "variables": {"a": acct, "s": a, "e": b}}).encode(), headers=H), timeout=120))
if v.get("errors"):
    print(json.dumps(v["errors"])[:800])
    sys.exit(1)
r = v["data"]["viewer"]["accounts"][0]
who = collections.Counter()
usd = collections.Counter()
for x in r["c"]:
    s = x["sum"]
    mem, cpu, disk = s["allocatedMemory"] / 2**30 * 0.0000025, s["cpuTimeSec"] * 0.000020, s["allocatedDisk"] / 1e9 * 0.00000007
    usd["containers"] += mem + cpu + disk
    who[apps.get(x["dimensions"]["applicationId"].replace("-", ""), "?")] += mem + cpu + disk
for x in r["p"]:
    s = x["sum"]
    rows = s["rowsWritten"] / 1e6 + s["rowsRead"] / 1e9
    dur = s["activeTime"] / 1e6 * 0.125 * 15 / 1e6  # the bill's rate came out near $15 per million GB-s
    usd["do_rows"] += rows
    usd["do_duration"] += dur
    who[ns.get(x["dimensions"]["namespaceId"], "?")] += rows + dur
for x in r["d"]:
    c = x["sum"]["requests"] * 0.15 / 1e6
    usd["do_requests"] += c
    who[x["dimensions"]["scriptName"]] += c
for x in r["w"]:
    c = x["sum"]["requests"] * 0.30 / 1e6 + x["sum"]["cpuTimeUs"] / 1000 * 0.02 / 1e6
    usd["workers"] += c
    who[x["dimensions"]["scriptName"]] += c
out = {"from": a, "to": b, **{k: round(x, 2) for k, x in usd.items()}, "usd": round(sum(usd.values()), 2)}
if os.environ.get("PREFIX"):
    out["prefix"] = os.environ["PREFIX"]
    out["prefix_usd"] = round(sum(x for k, x in who.items() if k.startswith(os.environ["PREFIX"])), 2)
print(json.dumps(out))
for k, x in who.most_common(5):
    print(f"  {k:40} ${x:.2f}")
