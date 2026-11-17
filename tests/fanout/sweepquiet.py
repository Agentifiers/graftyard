#!/usr/bin/env python3
"""sweepquiet.py FROM_ISO TO_ISO -> sweep-quiet's hosted check on an install (default graft-m2b, INSTALL to change).
Per 15-minute cron slot: rows written by `names:*` Member objects in the slot (durableObjectsPeriodicGroups per
minute), and each site's cron run from Workers Logs: its `sweep {` report (homesRead, networks swept, unresolved,
wall) or `sweep failed` line. Before sweep-quiet a slot's `names:*` rows were about 2.6 k and site a's sweep failed at
the subrequest limit (`tests/am1/results/lm1-memory.md`, sweep-quiet). Account and token from the environment, the
token only as a header."""
import collections, datetime, json, os, sys, urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
H = {"authorization": f"Bearer {tok}", "content-type": "application/json"}
P = os.environ.get("INSTALL", "graft-m2b")
a, b = sys.argv[1], sys.argv[2]
ms = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
hm = lambda t: datetime.datetime.fromtimestamp(t / 1000, datetime.UTC).strftime("%H:%M:%S")
slot = lambda t: t // 900_000 * 900_000


def api(url, body=None):
    r = urllib.request.Request(url, data=json.dumps(body).encode() if body else None, headers=H,
                               method="POST" if body else "GET")
    return json.load(urllib.request.urlopen(r, timeout=120))


ns = api(f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/durable_objects/namespaces?per_page=1000")
ids = [n["id"] for n in ns["result"] if (n.get("script") or "").startswith(P) and n.get("class") == "Member"]
q = """query($a:String!,$s:Time!,$e:Time!,$n:[string!]){viewer{accounts(filter:{accountTag:$a}){
  p: durableObjectsPeriodicGroups(limit:10000, filter:{datetime_geq:$s, datetime_lt:$e, namespaceId_in:$n}) {
    sum { rowsWritten } dimensions { name datetimeMinute } } }}}"""
rows = collections.Counter()
objs = collections.defaultdict(set)
t = ms(a)
while t < ms(b):  # an hour a query keeps each answer under the row limit
    e = min(ms(b), t + 3_600_000)
    iso = lambda x: datetime.datetime.fromtimestamp(x / 1000, datetime.UTC).strftime("%Y-%m-%dT%H:%M:%SZ")
    v = api("https://api.cloudflare.com/client/v4/graphql", {"query": q, "variables": {"a": acct, "s": iso(t), "e": iso(e), "n": ids}})
    if v.get("errors"):
        sys.exit(json.dumps(v["errors"])[:600])
    for x in v["data"]["viewer"]["accounts"][0]["p"]:
        d = x["dimensions"]
        if (d.get("name") or "").startswith("names:") and x["sum"]["rowsWritten"]:
            at = ms(d["datetimeMinute"])
            rows[slot(at)] += x["sum"]["rowsWritten"]
            objs[slot(at)].add(d["name"])
    t = e
runs = collections.defaultdict(list)
for site in "abc":
    t = ms(a)
    while t < ms(b):
        e = min(ms(b), t + 3_600_000)
        body = {"queryId": "q", "timeframe": {"from": t, "to": e}, "view": "events", "limit": 100,
                "parameters": {"filters": [
                    {"key": "$metadata.service", "operation": "eq", "type": "string", "value": f"{P}-site-{site}"},
                    {"key": "$metadata.message", "operation": "includes", "type": "string", "value": "sweep "}]}}
        ev = api(f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query", body)
        for x in ev.get("result", {}).get("events", {}).get("events", []):
            m = x.get("$metadata", {}).get("message") or ""
            if m.startswith("sweep {"):
                r = json.loads(m[6:])
                runs[slot(x["timestamp"])].append(
                    f"{site} {hm(x['timestamp'])} ok homesRead {r.get('homesRead', '-')} swept {len(r['swept'])} "
                    f"planned {sum(s['planned'] for s in r['swept'])} unresolved {len(r['unresolved'])} "
                    f"wall {(r['at'] - r['now']) // 1000} s")
            elif m.startswith("sweep failed"):
                runs[slot(x["timestamp"])].append(f"{site} {hm(x['timestamp'])} FAILED {m[14:90]}")
        t = e
for s in sorted(set(rows) | set(runs)):
    print(f"slot {hm(s)}: names:* rows {rows[s]:,} from {len(objs[s])} objects")
    for line in sorted(runs[s]):
        print(f"  {line}")
