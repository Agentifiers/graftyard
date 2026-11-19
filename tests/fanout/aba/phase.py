"""Leader confirm by phase (before load / load) and statements per flush, plus ls-refs reads anywhere before load.
Usage: phase.py TSAVE LOADSTART LOADEND"""
import json, sys, collections, datetime
P, A, B = sys.argv[1:4]
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
LA, LB = iso(A), iso(B)
pct = lambda xs, q: (sorted(xs)[min(len(xs) - 1, int(q * len(xs)))] if xs else None)
rows = collections.defaultdict(set)
for l in open(P):
    d = json.loads(l)
    if "rows" in d:
        rows[d.get("learner") if d.get("learner") is not None else f"v{d.get('site')}"].update(tuple(x) for x in d["rows"])
lead = rows["v0"]
fl = sorted((x[0], x[4]) for x in lead if x[1] == 16)
import bisect
ft = [f[0] for f in fl]
def stmts(t, ms):  # the flush closest before the confirm's end
    i = bisect.bisect_right(ft, t) - 1
    return fl[i][1] if i >= 0 and t - ft[i] <= ms + 50 else None
out = collections.defaultdict(list)
for x in lead:
    if x[1] != 10: continue
    ph = "pre" if x[0] < LA else ("load" if x[0] < LB else None)
    if ph is None: continue
    s = stmts(x[0], x[5]); g = "1" if s is not None and s <= 1 else ("2+" if s else "?")
    out[(ph, g)].append(x[5]); out[(ph, "all")].append(x[5])
first = min(x[0] for x in lead)
for k in sorted(out):
    print(f"leader {k[0]:4} stmts {k[1]:3}: n {len(out[k]):5} p50 {pct(out[k],.5)} p90 {pct(out[k],.9)} p99 {pct(out[k],.99)}")
pre_ls = sum(1 for m, v in rows.items() for x in v if x[1] == 26 and x[4] in (23, 25) and x[0] < LA)
print(f"trace starts {datetime.datetime.fromtimestamp(first/1000, datetime.timezone.utc):%H:%M:%S}; ls-refs/refs reads on any member before load: {pre_ls}")
for m in ("v1", "v2"):
    for ph, (a, b) in (("pre", (0, LA)), ("load", (LA, LB))):
        c = [x[5] for x in rows[m] if x[1] == 10 and a <= x[0] < b]
        print(f"  {m} {ph}: confirm n {len(c)} p50 {pct(c,.5)} p99 {pct(c,.99)}")
