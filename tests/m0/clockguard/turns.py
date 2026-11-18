#!/usr/bin/env python3
"""turns.py TSAVE LOAD_START_MS -> per member, the timing rows that show a blocked turn: ticker lag, step and persist,
transaction confirm, and the event-loop lag on each /raft send and handler (`tests/m0/results/clock-guard.md`).
"""
import json, lzma, sys, collections
# Leader-side timing rows in the load window: Lag (8, ticker late ms), Stepped (15, sync step+persist ms),
# Flushed (16), Confirm (10), CallSent (23, ms = loop lag), CallLeft (24, ms = loop lag at entry), per source.
path, t0 = sys.argv[1], int(sys.argv[2]); t1 = t0 + 600000
op = lzma.open if path.endswith('.xz') else open
seen = set(); v = collections.defaultdict(lambda: collections.defaultdict(list))
with op(path, 'rt') as f:
    for line in f:
        d = json.loads(line)
        if 'rows' not in d: continue
        src = f"l{d['learner']}" if 'learner' in d else f"v{d.get('site')}"
        for r in d['rows']:
            k = (src, tuple(r))
            if k in seen: continue
            seen.add(k)
            if t0 <= r[0] < t1 and r[1] in (8, 10, 15, 16, 23, 24):
                v[src][r[1]].append(r[5])
def pc(xs):
    xs = sorted(xs); n = len(xs)
    return f"n {n} sum {sum(xs)/1000:.0f}s p50 {xs[n//2]} p90 {xs[int(n*.9)]} p99 {xs[int(n*.99)]} max {xs[-1]}" if n else "n 0"
names = {8: 'Lag', 10: 'Confirm', 15: 'Stepped', 16: 'Flushed', 23: 'CallSent.lag', 24: 'CallLeft.lag'}
for s in sorted(v):
    for k in sorted(v[s]):
        print(s, names[k], pc(v[s][k]))
