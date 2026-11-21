"""Scratch: entry size against load. Round ~ commands in the entry + leader requests open at its append + followers'
requests open at /raft entry, all cuts in the steady window; then the same table two-way."""
import bisect, collections, json, lzma, sys
from datetime import datetime
import numpy as np
def ms(s): return int(datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp() * 1000)
def pc(xs, q):
    xs = sorted(xs); return round(xs[min(len(xs) - 1, int(q * len(xs)))]) if xs else None
path, a = sys.argv[1], ms(sys.argv[2]); lo, hi = a + 30000, a + 270000
rows, seen = collections.defaultdict(list), set()
for line in lzma.open(path, 'rt'):
    d = json.loads(line)
    if 'rows' not in d or 'learner' in d: continue
    for r in d['rows']:
        k = (d.get('site'),) + tuple(r)
        if k not in seen and a <= r[0] < a + 300000: seen.add(k); rows[d.get('site')].append(r)
R = sorted(rows[0], key=lambda r: r[0])
first, acks = {}, collections.defaultdict(list)
for pos, r in enumerate(R):
    if r[1] == 12 and r[2] < 192: first.setdefault(int(float(r[4])), (r[0], int(float(r[3])), pos))
    if r[1] == 14 and r[2] < 192 and int(float(r[4])) == 0: acks[int(float(r[3]))].append(r[0])
idxs = sorted(first, key=lambda i: first[i][2]); apos = [first[i][2] for i in idxs]
cmds = {}
for pos, r in enumerate(R):
    if r[1] == 29:
        k = bisect.bisect_left(apos, pos) - 1
        if k >= 0:
            bt = int(float(r[3])); cmds.setdefault(idxs[k] + (r[2] // 16) % 4 + 1, sum((bt >> (12 * i)) & 0xfff for i in range(4)))
opn = sorted((r[0], int(float(r[5]))) for r in R if r[1] == 25); ot = [x[0] for x in opn]
fo = {f: sorted((r[0], int(float(r[5]))) for r in rows[f] if r[1] == 25) for f in (1, 2)}
ft = {f: [x[0] for x in fo[f]] for f in (1, 2)}
X, y = [], []
for i in idxs:
    t, rnd, _ = first[i]
    ack = min((x for x in acks.get(rnd, []) if x >= t), default=None)
    if ack is None or i not in cmds or not lo <= t < hi: continue
    j = bisect.bisect_right(ot, t) - 1
    f_ = [fo[f][bisect.bisect_right(ft[f], t) - 1][1] for f in (1, 2) if bisect.bisect_right(ft[f], t) > 0]
    X.append([cmds[i], opn[j][1] if j >= 0 else 0, sum(f_) / max(1, len(f_))]); y.append(ack - t)
X, y = np.array(X, float), np.array(y, float)
Z = np.column_stack([np.ones(len(y)), X])
b, *_ = np.linalg.lstsq(Z, y, rcond=None)
print(f'n {len(y)}: round = {b[0]:.0f} + {b[1]:.2f} x commands + {b[2]:.2f} x leader open + {b[3]:.2f} x follower open; R2 {1 - (y - Z @ b).var() / y.var():.2f}; '
      f'r(commands, leader open) {np.corrcoef(X[:, 0], X[:, 1])[0, 1]:+.2f}; at the medians: commands {b[1] * np.median(X[:, 0]):.0f} ms, leader open {b[2] * np.median(X[:, 1]):.0f} ms, follower open {b[3] * np.median(X[:, 2]):.0f} ms')
# median regression is closer to what a p50 table shows: do the two-way table instead
q = np.quantile(X[:, 1], [.33, .66])
print('  round p50 (n): rows = commands in the entry, columns = leader open requests at its append, thirds', [int(v) for v in q])
for name, m in (('<=32', X[:, 0] <= 32), ('33-63', (X[:, 0] > 32) & (X[:, 0] < 64)), ('>=64', X[:, 0] >= 64)):
    cells = []
    for lo_, hi_ in ((-1, q[0]), (q[0], q[1]), (q[1], 1e9)):
        v = y[m & (X[:, 1] > lo_) & (X[:, 1] <= hi_)]
        cells.append(f'{pc(list(v), .5)} ({len(v)})')
    print(f'    cmds {name:5}: ' + ' | '.join(cells))
