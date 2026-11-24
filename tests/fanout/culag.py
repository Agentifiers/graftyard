#!/usr/bin/env python3
"""culag.py NET SCRIPT FROM_ISO TO_ISO LOAD_ISO RAW -> a git cache shard network's catch-up cost per pack and the home
shard's lag, from the shards' `shard catch-up stored <NET>:` lines (Workers Logs, SCRIPT, the site script). RAW is the
lines as JSON (`.json.xz`): fetched from the observability API when it does not exist, with the objects' names from
`durableObjectsPeriodicGroups`, and read from it after (token from the environment, as a header only).

Prints, per shard object (`git#0#net:<NET>` is the home, `git#0#pool:<n>` the pool shards of the same site):
  - pieces with stored packs, packs a piece, KB a pack;
  - per piece: wall (the piece's `ingest` lap less its `heads` or `cursor` lap), `pages_ms.room` (`make_room`), the
    part outside every page lap (the log page reads), `pages_ms.ingest` and `pages_ms.refs`;
  - per stored pack: wall, the ingest lap and the container's `index_ms`; and packs a second of wall, the shard's
    capacity while it runs flat out;
then G39's counters per shard (`room_calls`, stored packs `adopted`, `batches`), then the home's lag: per 30 s since LOAD_ISO, how far behind the first pool shard to reach the same log index it
reached each index, p50 and max. tests/am1/results/catchup-lag-design.md reads it for erDA and mrB.
"""
import collections, json, lzma, os, sys, urllib.request
from datetime import datetime

net, svc, a, b, load, raw = sys.argv[1:7]
ms = lambda s: int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
if not os.path.exists(raw):
    acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
    H = {"authorization": f"Bearer {tok}", "content-type": "application/json"}
    rows, t, B = [], ms(a), ms(b)
    while t < B:
        f = [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": svc},
             {"key": "$metadata.message", "operation": "includes", "type": "string",
              "value": f"shard catch-up stored {net}:"}]
        body = {"queryId": "q", "timeframe": {"from": t, "to": min(B, t + 30_000)}, "view": "events", "limit": 2000,
                "parameters": {"filters": f}}
        ev = json.load(urllib.request.urlopen(urllib.request.Request(
            f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
            data=json.dumps(body).encode(), headers=H, method="POST"), timeout=90))["result"]["events"]["events"]
        if len(ev) >= 2000:
            sys.exit(f"{t}: 2000 events, truncated")
        rows += [{"t": e["timestamp"], "msg": e["$metadata"]["message"]} for e in ev]
        t += 30_000
    ids = sorted({json.loads(r["msg"].split(": ", 1)[1])["obj"].split(":")[0] for r in rows})
    q = """query($a:String!,$s:Time!,$e:Time!,$ids:[string!]){viewer{accounts(filter:{accountTag:$a}){
     p: durableObjectsPeriodicGroups(limit: 1000, filter:{datetime_geq:$s, datetime_lt:$e, objectId_in:$ids}) {
     dimensions { name objectId } } }}}"""
    v = json.load(urllib.request.urlopen(urllib.request.Request("https://api.cloudflare.com/client/v4/graphql",
        data=json.dumps({"query": q, "variables": {"a": acct, "s": a, "e": b, "ids": ids}}).encode(), headers=H),
        timeout=60))
    names = {r["dimensions"]["objectId"]: r["dimensions"]["name"] for r in v["data"]["viewer"]["accounts"][0]["p"]}
    with lzma.open(raw, "wt") as o:
        json.dump({"names": names, "rows": rows}, o)
d = json.load(lzma.open(raw, "rt"))
names, rows = d["names"], d["rows"]
home = f"git#0#net:{net}"
S, seen = collections.defaultdict(collections.Counter), collections.defaultdict(list)
G = collections.defaultdict(collections.Counter)
for r in sorted(rows, key=lambda r: r["t"]):
    j = json.loads(r["msg"].split(": ", 1)[1])
    o = names.get(j["obj"].split(":")[0], j["obj"][:6])
    seen[o].append((r["t"], j["to"]))
    s, p, m = j["stored"], j["pages_ms"], dict(j["ms"])
    if not ms(load) <= r["t"] < ms(b):
        continue
    # G39's counters (a site from g39-catchup on): `make_room` calls per run, stored packs adopted from a validate's
    # aside, and calls that ingested more than one stored pack. An older site logs none.
    g = G[o]
    g["runs"] += 1; g["room_calls"] += j.get("room_calls", 0)
    g["adopted"] += s.get("adopted", 0); g["batches"] += s.get("batches", 0); g["has"] += "room_calls" in j
    if not s["packs"]:
        continue
    w = m.get("ingest", 0) - m.get("heads", m.get("cursor", 0))
    c = S[o]
    c["pieces"] += 1; c["packs"] += s["packs"]; c["bytes"] += s["bytes"]; c["wall"] += w
    c["room"] += p["room"]; c["ingest"] += p["ingest"]; c["refs"] += p["refs"]; c["index"] += s["index_ms"]
    c["read"] += w - p["room"] - p["ingest"] - p["refs"] - p["save"]
print("shard              pieces packs/piece KB/pack | per piece ms: wall room read ingest refs | "
      "per pack ms: wall ingest index | packs/s")
for o, c in sorted(S.items()):
    n, k = c["pieces"], c["packs"]
    print(f"{o:18s} {n:6d} {k / n:11.1f} {c['bytes'] / k / 1e3:7.1f} | {c['wall'] / n:17.0f} {c['room'] / n:4.0f} "
          f"{c['read'] / n:4.0f} {c['ingest'] / n:6.0f} {c['refs'] / n:4.0f} | {c['wall'] / k:12.1f} "
          f"{c['ingest'] / k:6.1f} {c['index'] / k:5.1f} | {1000 * k / c['wall']:7.1f}")
print("G39 counters, catch-up runs since load: shard runs room_calls adopted batches (none: the site logs no G39 fields)")
for o, g in sorted(G.items()):
    print(f"{o:18s} {g['runs']:5d} " + (f"{g['room_calls']:10d} {g['adopted']:7d} {g['batches']:7d}" if g["has"] else "none"))
first = {}
for o, s in seen.items():
    if o != home:
        for t, x in s:
            first[x] = min(first.get(x, t), t)
best, m = [], None
for x in sorted(first, reverse=True):
    m = first[x] if m is None else min(m, first[x])
    best.append((x, m))
best.reverse()
import bisect
bx, lag = [x for x, _ in best], collections.defaultdict(list)
for t, x in seen.get(home, []):
    i = bisect.bisect_left(bx, x)
    if i < len(bx) and t >= ms(load):
        lag[(t - ms(load)) // 30000].append((t - best[i][1]) / 1000)
print(f"{home} behind the first pool shard at the same index, s, by 30 s since {load}:")
for k in sorted(lag):
    v = sorted(lag[k])
    print(f"  +{k * 30:4d} s  n {len(v):3d}  p50 {v[len(v) // 2]:6.1f}  max {v[-1]:6.1f}")
