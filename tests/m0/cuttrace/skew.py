import json, lzma, sys, bisect
# The leader's /stats timings mix two clocks: a command's `at` is now_ms() (raw Date.now(), member.rs commit_all_held)
# while the append time `t` in `Host::note` and the apply time in `last_apply_at` are the member clock
# (`self.now()` in `step`). commitMs ends at `now_ms()` in `release` (raw) from `at` (raw): one clock.
# This rebuilds the member clock at each leader input from stamps it received (lower bound: edge stamps untraced) and
# replays queue_ms / queue:slot both ways, with commands taken from /commit arrivals (Entered 25, route 3) and the
# entry each rides modelled as the first append after arrival (W 0) or the second.
path, t0 = sys.argv[1], int(sys.argv[2]); t1 = t0 + 600000
seen = set(); ev = []
for l in lzma.open(path, 'rt'):
    d = json.loads(l)
    if 'rows' not in d or 'learner' in d or d.get('site') != 0: continue
    for r in d['rows']:
        t = tuple(r)
        if t in seen: continue
        seen.add(t)
        if t0 - 60000 <= r[0] < t1 + 5000: ev.append(t)
ev.sort()
floor = 0; mc_at = {}; app = {}; aply = []; arr = []
last = 0
for r in ev:
    at, k = r[0], r[1]
    if k in (20, 21): floor = max(floor, r[4])
    m = max(at, floor, last); last = m
    if k == 15: app.setdefault(r[4], (at, m))
    if k == 4: aply.append((at, m, r[4]))
    if k == 25 and r[4] % 64 == 3 and t0 <= at < t1: arr.append((at, m))
def pc(xs, q): xs = sorted(xs); return xs[min(len(xs) - 1, int(len(xs) * q))]
ents = sorted((v[0], v[1], i) for i, v in app.items())
starts = [e[0] for e in ents]
applies = sorted(aply)
ap_raw = [a[0] for a in applies]
mixed, raw = [], []
for e_raw, e_m in arr:
    j = bisect.bisect_right(starts, e_raw)
    if j + 1 >= len(ents): continue
    a_raw, a_m, _ = ents[j + 1]  # the second append after arrival, the usual one under read-ahead
    raw.append(a_raw - e_raw); mixed.append(a_m - e_raw)
print(f"commands {len(raw)}")
print(f"queue (append - admitted), one clock (raw):  p50 {pc(raw,.5)} p90 {pc(raw,.9)} p99 {pc(raw,.99)}")
print(f"queue as /stats measures it (member - raw): p50 {pc(mixed,.5)} p90 {pc(mixed,.9)} p99 {pc(mixed,.99)}")
lead = [m - a for a, m in arr]
print(f"member clock lead at /commit arrivals: p50 {pc(lead,.5)} p90 {pc(lead,.9)} p99 {pc(lead,.99)}")
