"""Scratch: does a leader storage confirm pending when an append is sent stretch that leg (the output gate holding the
fetch)? One clock: leg = CallSent -> AnswerArrived on the leader; `pending` = ms until the last leader sync() that had
begun before the send resolves. Entry-carrying voter calls, steady window, by commands in the entry."""
import bisect, collections, json, lzma, sys
from datetime import datetime
def ms(s): return int(datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp() * 1000)
def pc(xs, q):
    xs = sorted(xs); return round(xs[min(len(xs) - 1, int(q * len(xs)))]) if xs else None
path, a = sys.argv[1], ms(sys.argv[2]); lo, hi = a + 30000, a + 270000
rows, seen = [], set()
for line in lzma.open(path, 'rt'):
    d = json.loads(line)
    if 'rows' not in d or 'learner' in d or d.get('site') != 0: continue
    for r in d['rows']:
        k = tuple(r)
        if k not in seen and a <= r[0] < a + 300000: seen.add(k); rows.append(r)
L = sorted(rows, key=lambda r: r[0])
first = {}
for pos, r in enumerate(L):
    if r[1] == 12 and r[2] < 192: first.setdefault(int(float(r[4])), (r[0], pos))
idxs = sorted(first, key=lambda i: first[i][1]); apos = [first[i][1] for i in idxs]
cmds = {}
for pos, r in enumerate(L):
    if r[1] == 29:
        k = bisect.bisect_left(apos, pos) - 1
        if k >= 0:
            bt = int(float(r[3])); cmds.setdefault(idxs[k] + (r[2] // 16) % 4 + 1, sum((bt >> (12 * i)) & 0xfff for i in range(4)))
qt = sorted((first[i][0], cmds[i]) for i in idxs if i in cmds); qts = [x[0] for x in qt]
S = sorted((r[0] - int(float(r[5])), r[0]) for r in L if r[1] == 31)
sent = {(r[2], int(float(r[3]))): r[0] for r in L if r[1] == 23 and r[2] < 192 and lo <= r[0] < hi}
ans = {(r[2], int(float(r[3]))): r[0] for r in L if r[1] == 21 and r[2] < 192}
V = collections.defaultdict(list)
for (p, c), t in sent.items():
    if (p, c) not in ans: continue
    j = bisect.bisect_right(qts, t) - 1
    if j < 0 or t - qt[j][0] > 5: continue
    pend = max((e - t for b, e in S if b < t and e > t), default=0)
    V[(p, 'full' if qt[j][1] >= 64 else 'under 64')].append((pend, ans[(p, c)] - t))
for key in sorted(V):
    v = V[key]
    cells = []
    for lo_, hi_ in ((0, 0), (1, 50), (51, 150), (151, 500), (501, 99999)):
        w = [x for x in v if lo_ <= x[0] <= hi_]
        if len(w) >= 5:
            cells.append(f'pending {lo_}-{hi_ if hi_ < 99999 else ""}: leg p50 {pc([x[1] for x in w], .5)}, leg minus pending p50 {pc([x[1] - x[0] for x in w], .5)} (n {len(w)})')
    below = sum(1 for x in v if x[0] > 50 and x[1] < x[0])
    print(f'0->{key[0]} {key[1]:8}: ' + ' | '.join(cells) + f' | legs that ended BEFORE a >50 ms pending confirm did: {below} of {sum(1 for x in v if x[0] > 50)}')
