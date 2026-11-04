#!/usr/bin/env python3
"""BF-24's distribution from the driver's JSON lines: sum.py RUN.jsonl [...]. Latency is the push's return to the
first `git ls-remote` that shows refs/pull/N/merge right, with the leader's `mergeable` matching. A trial that never
did within the driver's 30 s counts as over 30 s, so it lands in p99 and max. p99 is the nearest-rank value. The
server time is the leader's clock from the push's entry to the head's `TestMerged` entry. Trials with a leader
change are listed apart."""
import json, math, sys

rows = [json.loads(l) for p in sys.argv[1:] for l in open(p) if l.strip()]
INF = float("inf")


def q(xs, p):
    xs = sorted(xs)
    return xs[max(0, math.ceil(p * len(xs)) - 1)] if xs else None


def fmt(x):
    return ">30" if x == INF else (None if x is None else round(x, 2))


steady = [r for r in rows if not (r.get("transfer") or r.get("lost"))]
for name, sel in [("clean", lambda r: not r["conflict"]), ("conflict", lambda r: r["conflict"]), ("all", lambda r: True)]:
    rs = [r for r in steady if sel(r)]
    lat = [INF if r["latency_s"] is None else r["latency_s"] for r in rs]
    fails = [r["i"] for r in rs if r["latency_s"] is None or r["latency_s"] > 10]
    srv = [r["server"]["ms"] / 1000 for r in rs if (r.get("server") or {}).get("ms") is not None]
    print(f"{name:8} n={len(rs):3}  within 10 s={len(rs) - len(fails):3}  p50={fmt(q(lat, .5))}  p99={fmt(q(lat, .99))}  "
          f"max={fmt(max(lat) if lat else None)}  over 10 s={fails}")
    print(f"{'':8} server, push entry to TestMerged: p50={fmt(q(srv, .5))}  p99={fmt(q(srv, .99))}  "
          f"max={fmt(max(srv) if srv else None)}")
for r in rows:
    if r.get("transfer") or r.get("lost"):
        t = r.get("transfer") or {}
        print(f"trial {r['i']} {'conflict' if r['conflict'] else 'clean'}: leader {t.get('from')}->{t.get('to')} "
              f"{'reset ' + str(t['reset_site']) + ' ' if 'reset_site' in t else ''}"
              f"lost={ {k: v for k, v in (r.get('lost') or {}).items() if k != 'site'} or None}  "
              f"latency={fmt(r['latency_s'])}  mergeable={r['mergeable']}  server={(r.get('server') or {}).get('ms')}")
