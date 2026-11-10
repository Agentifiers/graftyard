#!/usr/bin/env python3
"""memstats.py FROM_ISO TO_ISO REPO_ID -> one JSON line for a run's window on graft-m2b: each `repo:REPO_ID` member's
peak memory (durableObjectsPeriodicGroups max memoryUsageBytes, MiB), CPU and active seconds; the site scripts'
`exceededMemory` outcomes (Workers Logs, exact `calculations` view); and each member's inbound `/read` and `/suspect`
counts. Account and token from the environment, the token only as a header."""
import json, os, subprocess, sys, urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
a, b, rid = sys.argv[1:4]
H = {"authorization": f"Bearer {tok}", "content-type": "application/json"}
q = """query($a:String!,$s:Time!,$e:Time!,$n:String!){viewer{accounts(filter:{accountTag:$a}){
 p: durableObjectsPeriodicGroups(limit: 10, orderBy:[max_memoryUsageBytes_DESC], filter:{datetime_geq:$s, datetime_lt:$e, name_like:$n}) {
 max { memoryUsageBytes } sum { cpuTime activeTime } dimensions { name } } }}}"""
v = json.load(urllib.request.urlopen(urllib.request.Request("https://api.cloudflare.com/client/v4/graphql", data=json.dumps(
    {"query": q, "variables": {"a": acct, "s": a, "e": b, "n": f"repo:{rid}#%"}}).encode(), headers=H), timeout=60))
mem = {r["dimensions"]["name"]: {"mib": round(r["max"]["memoryUsageBytes"] / 2**20), "cpu_s": round(r["sum"]["cpuTime"] / 1e6, 1),
                                 "active_s": round(r["sum"]["activeTime"] / 1e6)} for r in v["data"]["viewer"]["accounts"][0]["p"]}
here = os.path.dirname(os.path.abspath(__file__))


def lines(*args):
    out = subprocess.run([sys.executable, *args], capture_output=True, text=True).stdout.splitlines()
    return [l.split(None, 1) for l in out if l.strip()]


outcomes = {}
for s in "abc":
    for n, k in lines(f"{here}/count.py", f"graft-m2b-site-{s}", a, b, "$workers.outcome"):
        if k.strip() in ("exceededMemory", "exception"):
            outcomes.setdefault(k.strip(), {})[s] = int(n)
calls = {}
for path in ("/read", "/suspect"):
    for n, k in lines(f"{here}/byobject.py", a, b, path):
        if k.strip().startswith(f"repo:{rid}#"):
            calls.setdefault(k.strip(), {})[path] = int(n)
print(json.dumps({"window": [a, b], "repo": rid, "memory": mem, "outcomes": outcomes, "calls": calls}))
