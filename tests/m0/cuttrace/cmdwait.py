import json, lzma, sys, bisect, collections
# Every leader trace row is on the isolate's raw clock (`Host::trace` -> now_ms()). Per /commit arrival on the leader
# (Entered 25, route code 3 = index % 64), the entries appended (Stepped 15) and applied (Applied 4) after it:
#   k = how many appends began after the arrival before the one that applied first at or after...
# Lower bound on a command's commit: it rides the first entry appended after it arrived, so commit >= that entry's
# apply - arrival. Split: wait (arrival -> that append) + round (append -> apply).
path, t0 = sys.argv[1], int(sys.argv[2]); t1 = t0 + 600000
seen = set(); arr = []; app = {}; aply = []
for l in lzma.open(path, 'rt'):
    d = json.loads(l)
    if 'rows' not in d or 'learner' in d or d.get('site') != 0: continue
    for r in d['rows']:
        t = tuple(r)
        if t in seen: continue
        seen.add(t)
        if not (t0 - 5000 <= r[0] < t1 + 5000): continue
        if r[1] == 25 and r[4] % 64 == 3 and t0 <= r[0] < t1: arr.append(r[0])
        if r[1] == 15: app.setdefault(r[4], r[0])
        if r[1] == 4: aply.append((r[0], r[4]))
aply.sort(); applied_at = {}; prev = 0
for at, idx in aply:
    for i in range(prev + 1, idx + 1): applied_at.setdefault(i, at)
    prev = max(prev, idx)
ents = sorted((at, i) for i, at in app.items() if i in applied_at)
starts = [a for a, _ in ents]
def pc(xs, q): xs = sorted(xs); return xs[min(len(xs) - 1, int(len(xs) * q))]
lb, wait, rnd, nxt = [], [], [], []
for e in sorted(arr):
    j = bisect.bisect_right(starts, e)
    if j >= len(ents) - 1: continue
    a, i = ents[j]; a2, i2 = ents[j + 1]
    lb.append(applied_at[i] - e); wait.append(a - e); rnd.append(applied_at[i] - a); nxt.append(applied_at[i2] - e)
print(f"/commit arrivals on the leader in load: {len(arr)}")
for name, xs in (('rides first append after arrival: commit', lb), ('  wait to that append', wait),
                 ('  its round', rnd), ('rides the second append: commit', nxt)):
    print(f"{name}: p50 {pc(xs,.5)} p90 {pc(xs,.9)} p99 {pc(xs,.99)}")
