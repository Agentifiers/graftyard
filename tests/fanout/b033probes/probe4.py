"""Scratch: what a round's length follows, per arm. OLS on standardized predictors, steady window.
    python3 probe4.py TRACE.jsonl.xz SITE FROM_ISO SECONDS
"""
import bisect, collections, json, lzma, sys
from datetime import datetime
import numpy as np


def ms(s):
    return int(datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp() * 1000)


path, s, a = sys.argv[1], int(sys.argv[2]), ms(sys.argv[3])
b = a + 1000 * int(sys.argv[4])
rows, seen = collections.defaultdict(list), set()
for line in lzma.open(path, 'rt'):
    d = json.loads(line)
    if 'rows' not in d or 'learner' in d:
        continue
    for r in d['rows']:
        k = (d.get('site'),) + tuple(r)
        if k not in seen and a - 3000 <= r[0] < b + 3000:
            seen.add(k)
            rows[d.get('site')].append(r)
R = sorted((r for r in rows[s] if a <= r[0] < b), key=lambda r: r[0])
first, acks = {}, collections.defaultdict(list)
for pos, r in enumerate(R):
    if r[1] == 12 and r[2] < 192:
        first.setdefault(int(float(r[4])), (r[0], int(float(r[3])), pos, int(float(r[5]))))
    if r[1] == 14 and r[2] < 192 and int(float(r[4])) == 0:
        acks[int(float(r[3]))].append(r[0])
idxs = sorted(first, key=lambda i: first[i][2])
apos = [first[i][2] for i in idxs]
cut = {}
for pos, r in enumerate(R):
    if r[1] != 29:
        continue
    k = bisect.bisect_left(apos, pos) - 1
    if k >= 0:
        batch = int(float(r[3]))
        cut.setdefault(idxs[k] + (r[2] // 16) % 4 + 1, sum((batch >> (12 * i)) & 0xfff for i in range(4)))
work = sorted((r[0], int(float(r[4])) & 0xfffff, int(float(r[5])) & 0xffff, (int(float(r[5])) >> 16) & 0xffff,
               int(float(r[5])) >> 32, r[2]) for r in R if r[1] == 30)
wt = [w[0] for w in work]
ent = sorted(r[0] for r in R if r[1] == 25)
lag = sorted((r[0], int(float(r[5]))) for r in R if r[1] == 8)
sync = sorted((r[0], int(float(r[5]))) for r in R if r[1] == 31)
fopen = {}
for f in (1, 2):
    fopen[f] = sorted((r[0], int(float(r[5]))) for r in rows[f] if r[1] == 25)
X, y, names = [], [], ['cmds in this entry', 'entry KB', 'leader stmts in the round', 'leader events+resolutions in the round',
                       'leader inputs in the round /s', 'leader ticker lag ms in the round', 'leader sync ms ending in the round',
                       'followers open requests (mean at append)']
lo, hi = a + 30000, a + 270000
for i in idxs:
    if i not in cut:
        continue
    t, rnd, _, nbytes = first[i]
    ack = min((x for x in acks.get(rnd, []) if x >= t), default=None)
    if ack is None or not lo <= t < hi:
        continue
    w = work[bisect.bisect_left(wt, t):bisect.bisect_right(wt, ack)]
    inputs = bisect.bisect_right(ent, ack) - bisect.bisect_left(ent, t)
    lg = sum(m for at, m in lag if t < at <= ack + 100)
    sy = sum(m for at, m in sync if t < at <= ack)
    fo = []
    for f in (1, 2):
        ts = [x[0] for x in fopen[f]]
        j = bisect.bisect_right(ts, t) - 1
        if j >= 0:
            fo.append(fopen[f][j][1])
    X.append([cut[i], nbytes / 1024, sum(x[1] for x in w), sum(x[3] + x[4] for x in w), 1000 * inputs / max(1, ack - t), lg,
              sy, sum(fo) / max(1, len(fo))])
    y.append(ack - t)
X, y = np.array(X, float), np.array(y, float)
print(f'n {len(y)} round mean {y.mean():.0f} sd {y.std():.0f}')
for j, nm in enumerate(names):
    print(f'  r(round, {nm}) = {np.corrcoef(X[:, j], y)[0, 1]:+.2f}   [predictor p50 {np.median(X[:, j]):.0f} p90 {np.percentile(X[:, j], 90):.0f}]')
# OLS without the rate-like and outcome-contaminated columns: cmds, followers open, ticker lag, sync
for cols in ((0, 7), (0, 7, 5, 6)):
    Z = np.column_stack([np.ones(len(y))] + [X[:, c] for c in cols])
    beta, *_ = np.linalg.lstsq(Z, y, rcond=None)
    res = y - Z @ beta
    print(f'  OLS round ~ ' + ' + '.join(f'{beta[k + 1]:.2f} x {names[c]}' for k, c in enumerate(cols)) +
          f' + {beta[0]:.0f}; R2 {1 - res.var() / y.var():.2f}')
# log-round quantiles by ticker lag present
m = X[:, 5] > 0
print(f'  rounds with a ticker stall row (>=20 ms) inside: {m.sum()} of {len(y)}, round p50 {np.median(y[m]) if m.any() else 0:.0f} '
      f'vs {np.median(y[~m]):.0f} without; slowest 5% of rounds with a stall inside: '
      f'{100 * m[y >= np.percentile(y, 95)].mean():.0f}%, with sync >= 200 ms inside: '
      f'{100 * (X[:, 6] >= 200)[y >= np.percentile(y, 95)].mean():.0f}% (all rounds {100 * (X[:, 6] >= 200).mean():.0f}%)')
