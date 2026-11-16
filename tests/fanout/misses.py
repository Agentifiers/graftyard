#!/usr/bin/env python3
"""misses.py A1_ROWS.jsonl FROM_ISO TO_ISO NET -> B0-34's clone-cache misses, each probe joined to its shard object's line.

The probes are a1.mjs's `b034.misses` rows (`at`, `ms` the probe's first byte, `compose`, `key` = `cloneKeyHex`). The
lines are the shard object's `shard miss <net> <CloneKey digest>: {...}` in Workers Logs (branch miss-log, a52436aa),
on every site script: `arrived` (epoch ms), then `ready_ms`, `only`, `caught_up_ms`, `headers_ms`, `first_ms` and
`answer`, all ms after arrival. A probe joins the line with its key whose `arrived` is nearest after its own start.

Per miss it prints the split the line allows:
  container   `headers_ms` minus the step before it (`caught_up_ms` for a stock miss, else `ready_ms`, else 0);
  transit     `first_ms` minus `headers_ms` (the shard object passing the first chunk on);
  edge+net    the probe's first byte minus `first_ms` minus the time from the probe's start to `arrived`.
Then counts by `answer` (composed, or stock with its reason) and p50/p99 of each part. Lines with no probe (another
caller's miss) and probes with no line are counted. Raw lines go to RAW (env) as JSON lines when set. The account and
token come from the environment, the token only as a header.
"""
import collections, datetime, json, os, re, sys, urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
H = {"authorization": f"Bearer {tok}", "content-type": "application/json"}
P = os.environ.get("INSTALL", "graft-m2b")
ms = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
a1, A, B, net = sys.argv[1], ms(sys.argv[2]), ms(sys.argv[3]), sys.argv[4]
# a1.mjs's rows file (tests/am1/raw/a1-b0-34-<run>.jsonl): the summary row is the one that carries `b034`.
probes = [r for r in map(json.loads, open(a1)) if isinstance(r, dict) and "b034" in r][-1]["b034"]["misses"]
needle = f"shard miss {net} "
line_re = re.compile(rf"shard miss {net} ([0-9a-f]{{64}}): (\{{.*\}})")
raw = open(os.environ["RAW"], "w") if os.environ.get("RAW") else None
lines = []
for site in "abc":
    a = A - 60_000
    while a < B:
        e2 = min(B + 180_000, a + 30_000)
        f = [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": f"{P}-site-{site}"},
             {"key": "$metadata.message", "operation": "includes", "type": "string", "value": needle}]
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
            v.update(site=site, key=m.group(1))
            lines.append(v)
            if raw:
                raw.write(json.dumps(v) + "\n")
        a = e2
# The same line may come back from overlapping windows.
lines = list({(l["key"], l.get("arrived"), l["site"]): l for l in lines}.values())
by_key = collections.defaultdict(list)
for l in lines:
    by_key[l["key"]].append(l)
used, rows, unjoined = set(), [], 0
for p in probes:
    s = ms(p["at"])
    cand = [l for l in by_key.get(p["key"], []) if id(l) not in used and (l.get("arrived") or 0) >= s - 2_000]
    if not cand:
        unjoined += 1
        continue
    l = min(cand, key=lambda l: l["arrived"])
    used.add(id(l))
    before = l.get("caught_up_ms") if l.get("caught_up_ms") is not None else (l.get("ready_ms") or 0)
    h, f1 = l.get("headers_ms"), l.get("first_ms")
    rows.append({"at": p["at"], "probe_ms": p["ms"], "probe_compose": p.get("compose"), "answer": l.get("answer"),
                 "site": l["site"], "to_shard": l["arrived"] - s,
                 "container": None if h is None else h - before,
                 "transit": None if h is None or f1 is None else f1 - h,
                 "edge_net": None if f1 is None else p["ms"] - f1 - (l["arrived"] - s),
                 "line": l})
pct = lambda xs, q: sorted(xs)[min(len(xs) - 1, int(q * len(xs)))] if xs else None
print(json.dumps({"probes": len(probes), "lines": len(lines), "joined": len(rows), "probes_without_line": unjoined,
                  "lines_without_probe": len(lines) - len(used)}))
print(json.dumps({"answers": collections.Counter(str(r["answer"]) for r in rows)}))
for part in ("probe_ms", "to_shard", "container", "transit", "edge_net"):
    for ans in sorted({str(r["answer"]) for r in rows}):
        xs = [r[part] for r in rows if str(r["answer"]) == ans and r[part] is not None]
        print(json.dumps({"part": part, "answer": ans, "n": len(xs), "p50": pct(xs, 0.5), "p99": pct(xs, 0.99),
                          "max": max(xs) if xs else None}))
for r in rows:
    print(json.dumps({k: v for k, v in r.items() if k != "line"} | {"only": r["line"].get("only")}))
