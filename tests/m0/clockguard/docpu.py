#!/usr/bin/env python3
"""docpu.py FROM_ISO TO_ISO REPO_ID [NAME_PART] -> each repo:REPO_ID member's CPU and active seconds, per minute for the one
named, from durableObjectsPeriodicGroups (`tests/m0/results/clock-guard.md`). Account and token from the environment.
"""
import json, os, sys, urllib.request
# Per Durable Object of repo:RID, per minute: cpuTime and activeTime (s), from durableObjectsPeriodicGroups.
acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
a, b, rid = sys.argv[1:4]
H = {"authorization": f"Bearer {tok}", "content-type": "application/json"}
q = """query($a:String!,$s:Time!,$e:Time!,$n:String!){viewer{accounts(filter:{accountTag:$a}){
 p: durableObjectsPeriodicGroups(limit: 10000, filter:{datetime_geq:$s, datetime_lt:$e, name_like:$n}) {
 sum { cpuTime activeTime } dimensions { name datetimeMinute } } }}}"""
v = json.load(urllib.request.urlopen(urllib.request.Request("https://api.cloudflare.com/client/v4/graphql", data=json.dumps(
    {"query": q, "variables": {"a": acct, "s": a, "e": b, "n": f"repo:{rid}#%"}}).encode(), headers=H), timeout=60))
if v.get("errors"): sys.exit(json.dumps(v["errors"])[:600])
rows = v["data"]["viewer"]["accounts"][0]["p"]
by = {}
for r in rows:
    n = r["dimensions"]["name"]; m = r["dimensions"]["datetimeMinute"][11:16]
    by.setdefault(n, {})[m] = (r["sum"]["cpuTime"] / 1e6, r["sum"]["activeTime"] / 1e6)
for n in sorted(by):
    tot = sum(c for c, _ in by[n].values()); act = sum(x for _, x in by[n].values())
    print(f"{n}: cpu {tot:.1f} s, active {act:.0f} s over {len(by[n])} min")
    if len(sys.argv) > 4 and sys.argv[4] in n:
        for m in sorted(by[n]): print("   ", m, "cpu %.1f s/min (%.0f%%), active %.0f" % (by[n][m][0], 100 * by[n][m][0] / 60, by[n][m][1]))
