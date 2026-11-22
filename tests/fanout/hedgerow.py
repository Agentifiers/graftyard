#!/usr/bin/env python3
"""hedgerow.py RUN [RAW_DIR] -> `validate-hedge`'s verdict lines from one B0-34 row's raws (`decisions/DECISIONS.md`,
"`validate-hedge`"): the kill-phase human push p99 against 3 s; every push that straddled the kill (started within 5 s
before the kill call and still running at it) with where it validated (`validate_site`, `validate_tries`,
`validate_hedge` in `server-timing`); human validates outside the kill that took more than one try; and each hostile
network's `/bench_seed` grants against `owner_share` (HOT_POOL - 1 = 6). RAW_DIR defaults to tests/am1/raw/.

With PAIRS=1 (and CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_API_TOKEN in the environment, the token sent only as a header),
also the hedged validates whose losing shard answered 200 too, the doubled work the hedge costs. Workers Logs stores
the URL's hex parameters (`headsum`, `trailer`) as "REDACTED", so tries are paired by time, not by hash: for each
hedged human push, the `GitShard` `/validate` events with no `class` (a human's) that started inside the push's
window on each site of SITES (default graft-m1-site-a,b,c). A pair is counted when two sites each logged one that
answered 200; a hedged push with a 200 on only one site had a losing try that failed, was still running when the
logs end, or was not logged. Human validates run about one a second, so a stray match is possible [inference].
"""
import datetime
import json
import re
import sys

run, raw = sys.argv[1], (sys.argv[2] if len(sys.argv) > 2 else "tests/am1/raw/")
rows = [json.loads(l) for l in open(f"{raw}a1-b0-34-{run}.jsonl") if l.strip()]
t0 = next(datetime.datetime.fromisoformat(r["t0"].replace("Z", "+00:00")).timestamp() * 1000 for r in rows if "t0" in r)
summary = next((r for r in rows if "byKind" in r), {})
# The kill call's start, t0 + killAt: the `shard kill` row is written when the call returns (mt16: + 2,992 ms).
kill_at = t0 + 1000 * (summary.get("b034", {}).get("killAt") or 300)
pct = lambda xs, p: sorted(xs)[min(len(xs) - 1, int(p * len(xs)))] if xs else None
timing = lambda h: {k: float(v) for k, v in re.findall(r"(\w+);dur=([\d.]+)", h or "")}

kill_kind = summary.get("byKind", {}).get("push:human@kill", {})
print(f"kill-phase push:human p99 {kill_kind.get('p99')} ms (n {kill_kind.get('n')}) against 3,000"
      f" ({'met' if (kill_kind.get('p99') or 9e9) <= 3000 else 'not met'})")
pushes = [json.loads(l) for l in open(f"{raw}a1-pushes-b0-34-{run}.jsonl") if l.strip()]
human = [p for p in pushes if p["kind"] == "human"]
# Straddling: started within 5 s before the kill call and still running at it, or started while the call ran (it
# returned at the `shard kill` row's `at`; mt17: + 3,992 ms).
kill_end = t0 + next((r["at"] for r in rows if r.get("step") == "shard kill"), kill_at - t0)
straddle = [p for p in human if (kill_at - 5000 <= p["start"] <= kill_at and p["end"] >= kill_at) or kill_at < p["start"] <= kill_end]
print(f"straddled the kill: {len(straddle)}")
for p in sorted(straddle, key=lambda p: p["start"]):
    t = timing(p["timing"])
    print(f"  start {p['start'] - kill_at:+.0f} ms, client {p['end'] - p['start']} ms, {p['result']}, validate"
          f" {t.get('validate')} on site {t.get('validate_site')}, tries {t.get('validate_tries')},"
          f" hedge {t.get('validate_hedge')}, edge total {t.get('total')}")
outside = [timing(p["timing"]) for p in human if not (kill_at - 5000 <= p["start"] <= kill_at + 60_000)]
outside = [t for t in outside if "validate" in t]
multi = [t for t in outside if t.get("validate_tries", 1) > 1]
print(f"human validates outside the kill: n {len(outside)}, p99 {pct([t['validate'] for t in outside], .99)} ms,"
      f" more than one try {len(multi)} ({100 * len(multi) / max(1, len(outside)):.1f}%; flip past 10%),"
      f" hedged {sum(t.get('validate_hedge', 0) == 1 for t in outside)}")
grants = [r for r in rows if r.get("step") == "hostile k"]
by = {}
for g in grants:
    by.setdefault(g["chain"], []).append(g)
for c, gs in by.items():
    last = gs[-1]
    print(f"hostile {c}: k asked, last k {last.get('k')}, grants {json.dumps(last.get('grants'))}")
slots = {}
for c, gs in by.items():
    for site, held in (gs[-1].get("grants") or []):
        slots[site] = slots.get(site, 0) + len(held or [])
print(f"hostile owner's pool slots per warm site {json.dumps(slots)} against owner_share 6")
print(f"kMax {json.dumps(summary.get('b034', {}).get('kMax') or summary.get('kMax'))}")

hedged = [p for p in human if timing(p["timing"]).get("validate_hedge") == 1]
print(f"hedged human validates: {len(hedged)} of {len(human)}")
import os
if os.environ.get("PAIRS"):
    import urllib.request
    acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
    sites = os.environ.get("SITES", "graft-m1-site-a,graft-m1-site-b,graft-m1-site-c").split(",")
    def events(svc, a, b):
        f = [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": svc},
             {"key": "$workers.event.path", "operation": "eq", "type": "string", "value": "/validate"}]
        body = {"queryId": "q", "timeframe": {"from": a, "to": b}, "view": "events", "limit": 100, "parameters": {"filters": f}}
        r = urllib.request.Request(f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
            data=json.dumps(body).encode(), method="POST", headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
        ev = json.load(urllib.request.urlopen(r, timeout=60)).get("result", {}).get("events", {}).get("events", [])
        return [(e["$workers"]["event"].get("response", {}).get("status"), e["$workers"].get("wallTimeMs")) for e in ev
                if "class" not in e["$workers"]["event"].get("search", {})]
    both = 0
    for p in hedged:
        per = {svc[-1]: events(svc, p["start"], p["end"]) for svc in sites}
        ok = [k for k, v in per.items() if any(st == 200 for st, _ in v)]
        both += len(ok) >= 2
        print(f"  {p['client']} start {p['start'] - kill_at:+.0f} ms: human validates by site {json.dumps(per)}")
    print(f"hedged validates whose losing shard also answered 200: {both} of {len(hedged)} (paired by time)")
