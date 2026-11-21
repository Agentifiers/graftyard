"""Scratch: what the leader's time between stamps follows. The isolate clock moves only on I/O, so the time from one
distinct stamp to the next is the work started at the first stamp plus any idle wait. OLS of that gap on what the first
stamp carried: requests entered by route, an apply (Work row: resolutions, events, statements), an append (commands in
its cut). Coefficients are ms per unit; `share` is coefficient x total count over the window's wall time."""
import bisect, collections, json, lzma, sys
from datetime import datetime
import numpy as np
def ms(s): return int(datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp() * 1000)
path, a = sys.argv[1], ms(sys.argv[2]); lo, hi = a + 30000, a + 270000
rows, seen = [], set()
for line in lzma.open(path, 'rt'):
    d = json.loads(line)
    if 'rows' not in d or 'learner' in d or d.get('site') != 0: continue
    for r in d['rows']:
        k = tuple(r)
        if k not in seen and lo <= r[0] < hi: seen.add(k); rows.append(r)
by = collections.defaultdict(list)
for r in rows: by[r[0]].append(r)
stamps = sorted(by)
names = ['/commit entered', '/outcome entered', '/raft entered', 'other request entered', 'resolutions answered', 'events applied',
         'statements queued (100s)', 'append sent (entry, per command)', 'append sent (per entry)', 'voter answer arrived', 'sync resolved']
X, y = [], []
for i in range(len(stamps) - 1):
    f = [0.0] * len(names)
    for r in by[stamps[i]]:
        k = r[1]
        if k == 25:
            route = int(float(r[4])) % 64
            f[{3: 0, 5: 1, 1: 2}.get(route, 3)] += 1
        elif k == 30:
            m = int(float(r[5])); f[4] += m >> 32; f[5] += (m >> 16) & 0xffff; f[6] += (int(float(r[4])) & 0xfffff) / 100
        elif k == 29:
            bt = int(float(r[3])); f[7] += sum((bt >> (12 * j)) & 0xfff for j in range(4)); f[8] += 1
        elif k == 21: f[9] += 1
        elif k == 31: f[10] += 1
    X.append(f); y.append(stamps[i + 1] - stamps[i])
X, y = np.array(X), np.array(y, float)
Z = np.column_stack([np.ones(len(y)), X])
b, *_ = np.linalg.lstsq(Z, y, rcond=None)
res = y - Z @ b
wall = y.sum()
print(f'stamps {len(y)}, wall {wall / 1000:.0f} s, R2 {1 - res.var() / y.var():.2f}; intercept {b[0]:.1f} ms a stamp = {100 * b[0] * len(y) / wall:.0f}% of wall (idle wait and untraced work)')
for j, nm in enumerate(names):
    tot = X[:, j].sum()
    print(f'  {nm:34} {b[j + 1]:7.2f} ms each x {tot:8.0f} = {b[j + 1] * tot / 1000:6.1f} s = {100 * b[j + 1] * tot / wall:3.0f}% of wall')
