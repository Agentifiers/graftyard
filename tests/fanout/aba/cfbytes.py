"""Leader confirm vs the bytes of the entries it covered. Usage: cfbytes.py TSAVE FROM TO"""
import json, sys, collections, datetime
P, F, T = sys.argv[1:4]
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
T0, T1 = iso(F), iso(T)
pct = lambda xs, q: (sorted(xs)[min(len(xs) - 1, int(q * len(xs)))] if xs else None)
lead = set()
for l in open(P):
    d = json.loads(l)
    if "rows" in d and d.get("site") == 0 and d.get("learner") is None:
        lead.update(tuple(x) for x in d["rows"] if T0 - 5000 <= x[0] < T1)
by = {}
for x in lead:
    if x[1] == 12: by[x[4]] = max(by.get(x[4], 0), x[5])  # last entry -> bytes
idx = sorted(by)
import bisect
def covered(prev, cur):  # bytes of entries in (prev, cur]
    i, j = bisect.bisect_right(idx, prev), bisect.bisect_right(idx, cur)
    return sum(by[k] for k in idx[i:j])
conf = sorted((x[0], x[4], x[5]) for x in lead if x[1] == 10)
prev = None; bins = collections.defaultdict(list); zero = []
for t, ix, ms in conf:
    if prev is not None and t >= T0:
        b = covered(prev, ix)
        if ix == prev or b == 0: zero.append(ms)
        else: bins[min(b // 25000, 8)].append(ms)
    prev = max(prev or 0, ix)
print(f"confirms covering no new entry (apply/other writes): n {len(zero)} p50 {pct(zero,.5)} p99 {pct(zero,.99)}")
for k in sorted(bins):
    print(f"  entry bytes {k*25}-{(k+1)*25}{'+' if k==8 else ''} kB: n {len(bins[k])} p50 {pct(bins[k],.5)} p90 {pct(bins[k],.9)} p99 {pct(bins[k],.99)}")
