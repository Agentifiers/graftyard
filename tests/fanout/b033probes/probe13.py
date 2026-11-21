"""Scratch: where the per-command part of a leg sits. Cross-clock parts (out = follower handler entry - leader CallSent;
back = leader AnswerArrived - follower CallLeft) carry an unknown clock offset, so only their DIFFERENCE between entry
sizes within one arm and pair is read. Steady window, entry-carrying voter calls."""
import bisect, collections, json, lzma, sys
from datetime import datetime
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
L = sorted(rows[0], key=lambda r: r[0])
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
sent = {(r[2], int(float(r[3]))): r[0] for r in L if r[1] == 23 and r[2] < 192 and lo <= r[0] < hi}
ans = {(r[2], int(float(r[3]))): r[0] for r in L if r[1] == 21 and r[2] < 192}
for f in (1, 2):
    left = {int(float(r[3])): (r[0], int(float(r[4]))) for r in rows[f] if r[1] == 24 and r[2] == 0}
    v = collections.defaultdict(list)
    for (p, c), t in sent.items():
        if p != f or (p, c) not in ans or c not in left: continue
        j = bisect.bisect_right(qts, t) - 1
        if j < 0 or t - qt[j][0] > 5: continue
        n = qt[j][1]
        b = 'small (<=32)' if n <= 32 else 'mid (33-63)' if n < 64 else 'full (>=64)'
        v[b].append((left[c][1] - t, left[c][0] - left[c][1], ans[(p, c)] - left[c][0], ans[(p, c)] - t))
    s = v['small (<=32)']
    if len(s) < 10: print(f'0->{f}: too few small entries'); continue
    base = [pc([x[k] for x in s], .5) for k in range(4)]
    print(f'0->{f}: small entries n {len(s)}: leg p50 {base[3]}')
    for b in ('mid (33-63)', 'full (>=64)'):
        w = v[b]
        m = [pc([x[k] for x in w], .5) for k in range(4)]
        print(f'    {b:12} n {len(w):3}: leg p50 {m[3]} (+{m[3] - base[3]} over small) = before the follower handler {m[0] - base[0]:+} | in the handler {m[1] - base[1]:+} | '
              f'after it returns {m[2] - base[2]:+}')
