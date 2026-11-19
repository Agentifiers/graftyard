"""Leader confirm at setup (trace start + 60 s up to the first 30 s with learner /read over 30/s) and under load, per trace.
Usage: pre2.py TSAVE..."""
import json, sys, collections, lzma
pct = lambda xs, q: (sorted(xs)[min(len(xs) - 1, int(q * len(xs)))] if xs else None)
for P in sys.argv[1:]:
    op = lzma.open if P.endswith(".xz") else open
    lead = set(); lr = collections.Counter(); ls = 0; L = None
    for l in op(P, "rt"):
        d = json.loads(l)
        if "rows" not in d: continue
        if d.get("learner") is None and d.get("site") == d.get("leader") and d.get("site") is not None:
            L = d["site"]; lead.update(tuple(x) for x in d["rows"] if x[1] == 10)
        elif d.get("learner") is not None:
            for x in d["rows"]:
                if x[1] == 25 and x[4] % 64 == 2: lr[x[0] // 30000] += 1
                if x[1] == 26 and x[4] in (23, 25): ls += 1
    if not lead: print(P, "no leader rows"); continue
    t0 = min(x[0] for x in lead)
    hot = sorted(b for b, n in lr.items() if n > 900)
    if not hot: print(P, "no load"); continue
    a, b = hot[0] * 30000, (hot[-1] + 1) * 30000
    pre = [x[5] for x in lead if t0 + 60000 <= x[0] < a]; load = [x[5] for x in lead if a + 30000 <= x[0] < b - 30000]
    print(f"{P.split('/')[-1]:28} leader site {L}: setup p50 {pct(pre,.5)} (n {len(pre)}) | load p50 {pct(load,.5)} p99 {pct(load,.99)} (n {len(load)}) | learner ls-refs+refs reads {ls}")
