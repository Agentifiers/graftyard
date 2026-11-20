#!/usr/bin/env python3
"""docpumin.py NAME FROM_ISO TO_ISO -> one Durable Object's CPU-seconds per minute and its object id, from
durableObjectsPeriodicGroups (`cpuTime`, microseconds) filtered by the object's name, such as `repo:65577#1#f0`.
B0-35's "f0's vCPU" line is each minute's CPU-seconds over 60. Account and token from the environment, the token only
as a header."""
import json, os, sys, urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
name, frm, to = sys.argv[1], sys.argv[2], sys.argv[3]
q = """query($a:String!,$s:Time!,$e:Time!,$n:String!){viewer{accounts(filter:{accountTag:$a}){
 p: durableObjectsPeriodicGroups(limit: 10000, filter:{datetime_geq:$s, datetime_lt:$e, name:$n}) {
  sum { cpuTime } dimensions { datetimeMinute objectId } } }}}"""
r = urllib.request.Request("https://api.cloudflare.com/client/v4/graphql", method="POST",
                           data=json.dumps({"query": q, "variables": {"a": acct, "s": frm, "e": to, "n": name}}).encode(),
                           headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
v = json.load(urllib.request.urlopen(r, timeout=90))
if v.get("errors"):
    sys.exit(json.dumps(v["errors"]))
# A name can match an object in another namespace (another site's script); keep the one that used the most CPU.
per = {}
for g in v["data"]["viewer"]["accounts"][0]["p"]:
    d = g["dimensions"]
    per.setdefault(d["objectId"], {})
    per[d["objectId"]][d["datetimeMinute"]] = per[d["objectId"]].get(d["datetimeMinute"], 0) + g["sum"]["cpuTime"] / 1e6
if not per:
    sys.exit("no object by that name")
obj = max(per, key=lambda o: sum(per[o].values()))
by = per[obj]
print("object", obj, "; others", {o[:8]: round(sum(x.values()), 1) for o, x in per.items() if o != obj})
for m in sorted(by):
    print(m[11:16], f"{by[m]:.1f} CPU-s", f"{by[m] / 60:.2f} vCPU")
if by:
    xs = [by[m] / 60 for m in sorted(by)]
    print(f"mean {sum(xs) / len(xs):.2f} peak {max(xs):.2f}")
