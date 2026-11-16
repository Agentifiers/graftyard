#!/usr/bin/env python3
"""idlerows.py FROM_ISO TO_ISO [TOP] -> which of an install's Durable Objects write SQLite rows, from
durableObjectsPeriodicGroups: rows written per hour per namespace (with its script and class), then the TOP objects
(default 30) over the window by rows written, with their names, namespace and active time. The install is graft-m2b
unless INSTALL names another. Account and token from the environment, the token only as a header."""
import collections, json, os, sys, urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
a, b = sys.argv[1], sys.argv[2]
top = int(sys.argv[3]) if len(sys.argv) > 3 else 30
P = os.environ.get("INSTALL", "graft-m2b")
H = {"authorization": f"Bearer {tok}", "content-type": "application/json"}
ns = json.load(urllib.request.urlopen(urllib.request.Request(
    f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/durable_objects/namespaces?per_page=1000",
    headers=H), timeout=60))["result"]
mine = {n["id"]: f"{n.get('script')}/{n.get('class')}" for n in ns if (n.get("script") or "").startswith(P)}


def gql(q, v):
    r = json.load(urllib.request.urlopen(urllib.request.Request("https://api.cloudflare.com/client/v4/graphql",
        data=json.dumps({"query": q, "variables": v}).encode(), headers=H), timeout=120))
    if r.get("errors"):
        sys.exit(json.dumps(r["errors"])[:800])
    return r["data"]["viewer"]["accounts"][0]


hourly = gql("""query($a:String!,$s:Time!,$e:Time!,$n:[string!]){viewer{accounts(filter:{accountTag:$a}){
  p: durableObjectsPeriodicGroups(limit:10000, filter:{datetime_geq:$s, datetime_lt:$e, namespaceId_in:$n}) {
    sum { rowsWritten activeTime } dimensions { namespaceId datetimeHour } } }}}""",
    {"a": acct, "s": a, "e": b, "n": list(mine)})["p"]
by = collections.defaultdict(dict)
for r in hourly:
    d = r["dimensions"]
    by[d["datetimeHour"]][mine[d["namespaceId"]]] = r["sum"]["rowsWritten"]
print("rows written per hour, by namespace (script/class):")
for h in sorted(by):
    row = by[h]
    print(f"  {h}  total {sum(row.values()):>9,}  " + ", ".join(f"{k} {v:,}" for k, v in sorted(row.items(), key=lambda x: -x[1]) if v))
objs = gql("""query($a:String!,$s:Time!,$e:Time!,$n:[string!]){viewer{accounts(filter:{accountTag:$a}){
  p: durableObjectsPeriodicGroups(limit:10000, orderBy:[sum_rowsWritten_DESC], filter:{datetime_geq:$s, datetime_lt:$e, namespaceId_in:$n}) {
    sum { rowsWritten activeTime } dimensions { namespaceId objectId name } } }}}""",
    {"a": acct, "s": a, "e": b, "n": list(mine)})["p"]
agg = collections.defaultdict(lambda: [0, 0, "", ""])
for r in objs:
    d = r["dimensions"]
    x = agg[d["objectId"]]
    x[0] += r["sum"]["rowsWritten"]; x[1] += r["sum"]["activeTime"]; x[2] = d.get("name") or x[2]; x[3] = mine[d["namespaceId"]]
writers = [v for v in agg.values() if v[0]]
print(f"objects writing rows in the window: {len(writers)}, rows {sum(v[0] for v in writers):,}")
for rows, act, name, n in sorted(writers, key=lambda v: -v[0])[:top]:
    print(f"  {rows:>9,} rows  {act / 1e6:>8.0f} s active  {n}  {name}")
fam = collections.Counter()
for rows, _, name, n in writers:
    fam[(n, (name or "?").split(":")[0].split("#")[0])] += rows
print("by namespace and name family:")
for (n, f), rows in fam.most_common(20):
    print(f"  {rows:>9,}  {n}  {f}")
