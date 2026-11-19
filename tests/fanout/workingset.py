#!/usr/bin/env python3
"""workingset.py FROM_ISO TO_ISO [GIT_BYTES] -> the daily working set from the shards' `shard stats` lines (G1).

Each running git cache shard logs one `shard stats <object name>: {...}` line a day from its alarm
(`graftyard_git::shard::stats_line`): every repository on its disk with `bytes`, `pack_bytes`, and the last `used`,
`fetched`, `pushed`, and `caught_up` in epoch ms, plus `day` totals of those fetched, pushed, caught up, and used in the
24 h before the line. This reads every such line in [FROM, TO] on the install's three site scripts from Workers Logs,
keeps the newest line per (site, object), and prints per site and in all:

  held        networks and bytes on the shards' disks;
  fetched     networks fetched by a client in the day, and their bytes: the working set `scripts/cost.py` guesses
              (`WORKING_SET`); a network warm on two sites counts once per site, as its shards do;
  pushed, caught_up, used   the same for the other kinds.

With GIT_BYTES (the install's stored git bytes, e.g. from the pack buckets), it prints the fetched share of them,
the figure `cost.py` guesses at 30%, 20%, and 10% by tier. A test install measures the mechanics (residency,
on-disk size against stored packs), not a real fleet's share. INSTALL (env) names the install, default graft-m2b;
the account and token come from the environment, the token only as a header. Raw lines go to RAW (env) when set.
"""
import collections, datetime, json, os, re, sys, urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
H = {"authorization": f"Bearer {tok}", "content-type": "application/json"}
P = os.environ.get("INSTALL", "graft-m2b")
ms = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
A, B = ms(sys.argv[1]), ms(sys.argv[2])
git_bytes = int(sys.argv[3]) if len(sys.argv) > 3 else None
line_re = re.compile(r"shard stats (\S+): (\{.*\})")
raw = open(os.environ["RAW"], "w") if os.environ.get("RAW") else None
newest = {}
for site in "abc":
    a = A
    while a < B:
        e2 = min(B, a + 6 * 3_600_000)
        f = [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": f"{P}-site-{site}"},
             {"key": "$metadata.message", "operation": "includes", "type": "string", "value": "shard stats "}]
        body = {"queryId": "q", "timeframe": {"from": a, "to": e2}, "view": "events", "limit": 2000,
                "parameters": {"filters": f}}
        req = urllib.request.Request(
            f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
            data=json.dumps(body).encode(), headers=H, method="POST")
        for _ in range(4):
            try:
                ev = json.load(urllib.request.urlopen(req, timeout=90)).get("result", {}).get("events", {}).get("events", [])
                break
            except Exception as x:
                print(f"site {site} {a}: {x}, retry", file=sys.stderr)
        else:
            sys.exit(f"site {site} {a}: the query failed 4 times")
        if len(ev) >= 2000:
            print(f"site {site} {a}: 2000 events, truncated", file=sys.stderr)
        for e in ev:
            m = line_re.search(str(e.get("$metadata", {}).get("message", "")))
            if not m:
                continue
            v = json.loads(m.group(2))
            v.update(site=site, object=m.group(1))
            if raw:
                raw.write(json.dumps(v) + "\n")
            k = (site, m.group(1))
            if k not in newest or v["at"] > newest[k]["at"]:
                newest[k] = v
        a = e2
gb = lambda b: f"{b / 1e9:,.2f} GB"
kinds = ("fetched", "pushed", "caught_up", "used")
sums = collections.defaultdict(lambda: collections.Counter())
for (site, _), v in newest.items():
    for s in (site, "all"):
        sums[s]["shards"] += 1
        sums[s]["held_n"] += v["held"]["networks"]
        sums[s]["held_b"] += v["held"]["bytes"]
        for k in kinds:
            sums[s][k + "_n"] += v["day"][k]["networks"]
            sums[s][k + "_b"] += v["day"][k]["bytes"]
print("| Site | Shards | Held | " + " | ".join(kinds) + " |")
print("| --- | --- | --- | " + " | ".join("---" for _ in kinds) + " |")
for s in sorted(sums, key=lambda s: (s == "all", s)):
    c = sums[s]
    cells = [f"{c[k + '_n']} ({gb(c[k + '_b'])})" for k in kinds]
    print(f"| {s} | {c['shards']} | {c['held_n']} ({gb(c['held_b'])}) | " + " | ".join(cells) + " |")
if git_bytes and newest:
    print(f"\nfetched in the day, all sites: {sums['all']['fetched_b'] / git_bytes:.1%} of {gb(git_bytes)} stored")
if not newest:
    print("no `shard stats` lines in the window", file=sys.stderr)
