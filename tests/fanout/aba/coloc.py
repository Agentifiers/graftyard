"""Per-second leader confirm/loop lag vs learner lsRefs reads, split by learner site. Usage: coloc.py TSAVE FROM TO"""
import json, sys, collections, datetime
P, F, T = sys.argv[1:4]
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
T0, T1 = iso(F), iso(T)
LSREFS = 24  # READ_QUERIES index of lsRefs + 1
SITE = {192: 0, 193: 1, 194: 2, 195: 0, 196: 1, 197: 2, 198: 0, 199: 0}
pct = lambda xs, q: (sorted(xs)[min(len(xs) - 1, int(q * len(xs)))] if xs else None)
rows = collections.defaultdict(set)  # (site, learner) -> rows
for l in open(P):
    d = json.loads(l)
    if "rows" not in d:
        continue
    rows[(d.get("site"), d.get("learner"))].update(tuple(x) for x in d["rows"] if T0 <= x[0] < T1)
lead = rows[(0, None)]
sec = lambda t: (t - T0) // 1000
N = (T1 - T0) // 1000
conf = collections.defaultdict(list); lag = collections.defaultdict(list); appfail = collections.Counter()
alive = collections.defaultdict(list)
for x in lead:
    if x[1] == 10: conf[sec(x[0])].append(x[5])
    if x[1] in (23, 24): lag[sec(x[0])].append(x[5])
    if x[1] == 14 and x[4] == 1: appfail[(x[2])] += 1
# learner lsRefs reads per second by learner site. learner key: (site, learner addr)
ls = {}
lsite = {}
for (site, lrn), v in rows.items():
    if lrn is None: continue
    ent = {x[4] // 64: x[0] for x in v if x[1] == 25 and x[4] % 64 == 2}
    c = collections.Counter(sec(ent[x[3]]) for x in v if x[1] == 26 and x[4] in (23, 25) and x[3] in ent)
    ls[(SITE[lrn], lrn)] = c
print("learners lsRefs/s:", {f"{k[1]}@{k[0]}": round(sum(c.values()) / N, 2) for k, c in sorted(ls.items(), key=lambda kv: (kv[0][0], kv[0][1]))})
bysite = {s: collections.Counter() for s in (0, 1, 2)}
for (site, lrn), c in ls.items(): bysite[site].update(c)
# correlation per second
import statistics as st
xs_a = [bysite[0][i] for i in range(N)]; xs_bc = [bysite[1][i] + bysite[2][i] for i in range(N)]
yc = [st.median(conf[i]) if conf[i] else None for i in range(N)]
yl = [max(lag[i]) if lag[i] else None for i in range(N)]
def corr(a, b):
    p = [(u, v) for u, v in zip(a, b) if v is not None]
    if len(p) < 3: return None
    u, v = zip(*p)
    try: return round(st.correlation(u, v), 3)
    except Exception: return None
print(f"secs {N}; site-a learner lsRefs/s mean {sum(xs_a)/N:.1f}, b+c {sum(xs_bc)/N:.1f}")
print("corr(confirm p50/s, site-a lsRefs/s)", corr(xs_a, yc), " corr(confirm, b+c)", corr(xs_bc, yc))
print("corr(loop lag max/s, site-a)", corr(xs_a, yl), " corr(lag, b+c)", corr(xs_bc, yl))
# confirm by tercile of site-a lsRefs rate
qs = sorted(xs_a); lo, hi = qs[N // 3], qs[2 * N // 3]
for name, sel in (("low", lambda v: v <= lo), ("mid", lambda v: lo < v <= hi), ("high", lambda v: v > hi)):
    cs = [c for i in range(N) if sel(xs_a[i]) for c in conf[i]]
    lg = [g for i in range(N) if sel(xs_a[i]) for g in lag[i]]
    print(f"site-a lsRefs {name} (<= {lo}/{hi}): confirm n {len(cs)} p50 {pct(cs,.5)} p99 {pct(cs,.99)}; loop lag p50 {pct(lg,.5)} p99 {pct(lg,.99)}")
alllag = [g for v in lag.values() for g in v]
print(f"leader loop lag (CallSent/CallLeft) n {len(alllag)} p50 {pct(alllag,.5)} p90 {pct(alllag,.9)} p99 {pct(alllag,.99)}")
print("leader AppendEnded failed by peer:", dict(appfail))
# leader appends: issued->ended time to learners vs voters, and queued->issued
q = {}; iss = {}; endd = collections.defaultdict(list); qi = collections.defaultdict(list)
for x in sorted(lead):
    if x[1] == 12: q[(x[2], x[3], x[4])] = x[0]
    elif x[1] == 13: qi[x[2]].append(x[5])
    elif x[1] == 14: endd[x[2]].append(x[5])
for p in sorted(endd):
    print(f"  append to {p}: n {len(endd[p])} end p50 {pct(endd[p],.5)} p99 {pct(endd[p],.99)}; queued->issued p99 {pct(qi[p],.99)}")
# leader inbound open at entry (Entered ms) per second vs site-a
op = collections.defaultdict(list)
for x in lead:
    if x[1] == 25: op[sec(x[0])].append(x[5])
yo = [max(op[i]) if op[i] else None for i in range(N)]
print("corr(leader open max/s, site-a lsRefs)", corr(xs_a, yo))
