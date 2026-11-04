#!/usr/bin/env python3
"""BF-24's distribution from the driver's JSON lines: sum.py RUN.jsonl [...]. Latency is the push's return to the
first `git ls-remote` that shows refs/pull/N/merge right, with the leader's `mergeable` matching; a trial that never
did within 30 s is a fail. p99 is the nearest-rank value."""
import json, math, sys

rows = [json.loads(l) for p in sys.argv[1:] for l in open(p) if l.strip()]


def q(xs, p):
    xs = sorted(xs)
    return xs[max(0, math.ceil(p * len(xs)) - 1)] if xs else None


for name, sel in [("clean", lambda r: not r["conflict"]), ("conflict", lambda r: r["conflict"]), ("all", lambda r: True)]:
    rs = [r for r in rows if sel(r)]
    lat = [r["latency_s"] for r in rs if r["latency_s"] is not None]
    fails = [r["i"] for r in rs if r["latency_s"] is None or r["latency_s"] > 10]
    print(f"{name:8} n={len(rs):3}  passed={len(rs) - len(fails):3}  p50={q(lat, .5)}  p99={q(lat, .99)}  "
          f"max={max(lat) if lat else None}  fails={fails}")
    srv = [r["server"]["ms"] / 1000 for r in rs if (r.get("server") or {}).get("ms") is not None]
    print(f"{'':8} server push entry to TestMerged: p50={q(srv, .5)}  p99={q(srv, .99)}  max={max(srv) if srv else None}")
for r in rows:
    if r.get("transfer") or r.get("lost"):
        t = r.get("transfer") or {}
        print(f"trial {r['i']}: transfer {t.get('from')}->{t.get('to')} {t.get('r') or t.get('error')}  lost={r.get('lost')}  "
              f"latency={r['latency_s']}  mergeable={r['mergeable']}  server={(r.get('server') or {}).get('ms')}")
