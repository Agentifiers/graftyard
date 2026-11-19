"""Item 5: each REST merge's `tier_ahead` against what the voters saw. Pairs a1's mergeSteps (server-timing) with the
edge's Workers Logs merge events (start = ts - wall) by total, then lists the edge-isolate `/read` q3 calls with
SentBy leader or walk on s0/s1/s2 inside [start, start + tier_ahead + 50], and the leader's confirms there.
Usage: tierahead.py A1ROW TSAVE MERGEWL"""
import bisect, collections, json, lzma, re, sys, datetime
A1, P, M = sys.argv[1:4]
hms = lambda t: datetime.datetime.fromtimestamp(t / 1000, datetime.timezone.utc).strftime("%H:%M:%S.%f")[:12]
g = lambda m, k: int(re.search(k + r";dur=(\d+)", m["timing"]).group(1)) if m["timing"] and k + ";" in m["timing"] else None
steps = next(json.loads(l)["mergeSteps"] for l in open(A1) if '"mergeSteps"' in l)
wl = sorted((x for x in (json.loads(l) for l in open(M)) if x["wall"] is not None), key=lambda x: x["ts"])
pairs, j = [], 0
for m in steps:
    t = g(m, "total"); k = j
    while k < len(wl) and not (t is not None and abs(wl[k]["wall"] - t) <= 60): k += 1
    if k < len(wl): pairs.append((m, wl[k])); j = k + 1
seen = set(); ent = collections.defaultdict(dict); qry = collections.defaultdict(dict); by = collections.defaultdict(dict); conf = []
with (lzma.open if P.endswith(".xz") else open)(P, "rt") as f:
    for l in f:
        d = json.loads(l)
        if "rows" not in d or d.get("learner") is not None: continue
        who = "s" + str(d.get("site"))
        for r in d["rows"]:
            t = (who,) + tuple(r)
            if t in seen: continue
            seen.add(t)
            if r[1] == 25: ent[who][r[4] // 64] = (r[0], r[2], r[4] % 64)
            elif r[1] == 26: qry[who][r[3]] = r[4]
            elif r[1] == 27: by[who][r[3]] = (r[4], r[5])
            elif r[1] == 10 and who == "s0": conf.append((r[0], r[5]))
calls = []
for who in ent:
    for n, (t, peer, route) in ent[who].items():
        sb = by[who].get(n)
        if route == 2 and peer == 255 and qry[who].get(n) == 3 and sb and sb[1] in (1, 2):
            calls.append((t, who, "leader" if sb[1] == 1 else "walk", sb[0] % 100000))
calls.sort(); ct = [c[0] for c in calls]; conf.sort(); cf = [c[0] for c in conf]
print(f"{len(pairs)} of {len(steps)} merges paired")
slow, fast = [], []
for m, x in pairs:
    ta = g(m, "tier_ahead")
    if ta is None: continue
    a = x["ts"] - x["wall"]; b = a + ta + 50
    cs = calls[bisect.bisect_left(ct, a):bisect.bisect_right(ct, b)]
    walks = sum(c[2] == "walk" for c in cs)
    cmax = max([c[1] for c in conf[bisect.bisect_left(cf, a):bisect.bisect_right(cf, b)]] or [0])
    (slow if ta >= 400 else fast).append((ta, walks, cmax))
    if ta >= 400:
        print(f"  {hms(a)} {x['colo']} tier_ahead {ta}: " + ", ".join(f"+{c[0] - a} {c[1]} {c[2]} iso{c[3]}" for c in cs) + f"; leader confirm max {cmax}")
print(f"tier_ahead >= 400: {len(slow)}, with a walk in the span {sum(1 for s in slow if s[1])}")
print(f"tier_ahead < 400: {len(fast)}, with a walk in the span {sum(1 for s in fast if s[1])}; p50 {sorted(s[0] for s in fast)[len(fast) // 2] if fast else None}")
