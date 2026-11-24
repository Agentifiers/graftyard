"""Storage-contention split. Usage: stor.py TSAVE FROM TO
Per member: own Confirm p50/p99. Leader confirms binned by site-a / b+c learner lsRefs reads entered during the confirm."""
import os, sys, collections, datetime, bisect
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..'))
from tracerows import load
P, F, T = sys.argv[1:4]
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
T0, T1 = iso(F), iso(T)
SITE = {192: 0, 193: 1, 194: 2, 195: 0, 196: 1, 197: 2, 198: 0, 199: 0}
pct = lambda xs, q: (sorted(xs)[min(len(xs) - 1, int(q * len(xs)))] if xs else None)
rows = collections.defaultdict(list, {w[1] if isinstance(w, tuple) else f"v{w}": rs for w, rs in
                                     load(P, None, keep=lambda x: T0 - 5000 <= x[0] < T1).items()})
for m in sorted(rows, key=str):
    c = [x[5] for x in rows[m] if x[1] == 10 and x[0] >= T0]
    reads = sum(1 for x in rows[m] if x[1] == 25 and x[4] % 64 == 2 and x[0] >= T0)
    ls = sum(1 for x in rows[m] if x[1] == 26 and x[4] in (23, 25) and x[0] >= T0)
    print(f"{m} site {SITE.get(m, m)}: confirm n {len(c)} p50 {pct(c,.5)} p90 {pct(c,.9)} p99 {pct(c,.99)}; /read {reads/((T1-T0)/1000):.1f}/s lsRefs {ls/((T1-T0)/1000):.1f}/s")
ts = {0: [], 1: []}
for m, v in rows.items():
    if isinstance(m, int):
        ts[0 if SITE[m] == 0 else 1] += [x[0] for x in v if x[1] == 26 and x[4] in (23, 25)]
for k in ts: ts[k].sort()
cnt = lambda a, lo, hi: bisect.bisect_right(a, hi) - bisect.bisect_left(a, lo)
bins = collections.defaultdict(list)
for x in rows["v0"]:
    if x[1] != 10 or x[0] < T0: continue
    t, ms = x[0], x[5]
    a = cnt(ts[0], t - ms, t); b = cnt(ts[1], t - ms, t)
    rate = a * 1000 / max(ms, 1)
    bins[("a", min(a, 3))].append(ms); bins[("bc", min(b, 3))].append(ms)
for k in sorted(bins):
    print(f"leader confirms with {k[0]} lsRefs reads in window {k[1]}{'+' if k[1]==3 else ''}: n {len(bins[k])} p50 {pct(bins[k],.5)} p99 {pct(bins[k],.99)}")
# length-free: reads in the fixed 300 ms before the confirm started
for W in (300, 1000):
    fb = collections.defaultdict(list)
    for x in rows["v0"]:
        if x[1] != 10 or x[0] < T0: continue
        s0 = x[0] - x[5]
        a = cnt(ts[0], s0 - W, s0); b = cnt(ts[1], s0 - W, s0)
        q = W // 100
        fb[("a", min(a // max(1, q // 3), 4))].append(x[5]); fb[("bc", min(b // max(1, q // 3), 4))].append(x[5])
    for k in sorted(fb):
        print(f"W{W} before start, {k[0]} bucket {k[1]}: n {len(fb[k])} p50 {pct(fb[k],.5)} p99 {pct(fb[k],.99)}")
