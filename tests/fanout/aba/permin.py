"""Per 30 s across the whole trace: leader confirm p50, flushes, learner lsRefs+refs reads, leader inbound. Usage: permin.py TSAVE"""
import json, sys, collections, datetime
P = sys.argv[1]
pct = lambda xs, q: (sorted(xs)[min(len(xs) - 1, int(q * len(xs)))] if xs else None)
lead = set(); lr = set()
for l in open(P):
    d = json.loads(l)
    if "rows" not in d: continue
    if d.get("site") == 0 and d.get("learner") is None: lead.update(tuple(x) for x in d["rows"])
    elif d.get("learner") is not None: lr.update((d["learner"],) + tuple(x) for x in d["rows"])
B = 30000
cf = collections.defaultdict(list); fl = collections.Counter(); inb = collections.Counter(); ls = collections.Counter(); lrd = collections.Counter()
for x in lead:
    b = x[0] // B
    if x[1] == 10: cf[b].append(x[5])
    elif x[1] == 16: fl[b] += 1
    elif x[1] == 25: inb[b] += 1
for x in lr:
    b = x[1] // B
    if x[2] == 26 and x[5] in (23, 25): ls[b] += 1
    if x[2] == 25 and x[5] % 64 == 2: lrd[b] += 1
for b in sorted(set(cf) | set(ls)):
    t = datetime.datetime.fromtimestamp(b * B / 1000, datetime.timezone.utc).strftime("%H:%M:%S")
    c = cf[b]
    print(f"{t} confirm n {len(c):4} p50 {pct(c,.5)} p90 {pct(c,.9)} | flushes {fl[b]:4} leader in/s {inb[b]/30:6.1f} | learner /read/s {lrd[b]/30:6.1f} lsRefs+refs/s {ls[b]/30:5.1f}")
