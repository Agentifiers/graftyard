"""Scratch: per entry-carrying voter call, same-clock pieces: leg (leader clock), follower handler (follower clock),
follower storage confirm pending when/after its handler returned (follower clock), leader confirm pending at send."""
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
def syncs(R): return sorted((r[0] - int(float(r[5])), r[0]) for r in R if r[1] == 31)   # (begin, end)
def flushes(R): return sorted((r[0], int(float(r[5])), int(float(r[4]))) for r in R if r[1] == 16)  # (ran at, late ms, stmts)
LS = syncs(L)
def pending_until(S, t, slack=0):
    # the latest end among syncs that began at or before t+slack and end after t
    e = [end for b, end in S if b <= t + slack and end > t]
    return max(e) - t if e else 0
sent = {(r[2], int(float(r[3]))): r[0] for r in L if r[1] == 23 and r[2] < 192 and lo <= r[0] < hi}
ans = {(r[2], int(float(r[3]))): r[0] for r in L if r[1] == 21 and r[2] < 192}
out = []
for f in (1, 2):
    F = rows[f]; FS = syncs(F); FF = flushes(F)
    left = {int(float(r[3])): (r[0], int(float(r[4]))) for r in F if r[1] == 24 and r[2] == 0}
    fb = [x[0] for x in FS]
    for (p, c), t in sent.items():
        if p != f or (p, c) not in ans or c not in left: continue
        j = bisect.bisect_right(qts, t) - 1
        if j < 0 or t - qt[j][0] > 5: continue
        ret, entry = left[c]
        # follower: first sync that begins at/after handler entry (the confirm covering this append), end - return
        k = bisect.bisect_left(fb, entry)
        fgate = max(0, FS[k][1] - ret) if k < len(FS) and FS[k][0] <= ret + 300 else 0
        fgate_pending = pending_until(FS, ret)
        lgate = pending_until(LS, t)
        leg = ans[(p, c)] - t
        out.append({'f': f, 'n': qt[j][1], 'leg': leg, 'h': ret - entry, 'fgate': fgate, 'fpend': fgate_pending, 'lgate': lgate})
def show(name, v):
    g = lambda k, q=.5: pc([x[k] for x in v], q)
    rem = [x['leg'] - x['h'] - x['fgate'] - x['lgate'] for x in v]
    print(f'  {name:22} n {len(v):3} | leg p50 {g("leg")} p90 {g("leg", .9)} | follower handler {g("h")} / {g("h", .9)} | follower confirm after return {g("fgate")} / {g("fgate", .9)} '
          f'| leader confirm pending at send {g("lgate")} / {g("lgate", .9)} | leg minus all three p50 {pc(rem, .5)} p90 {pc(rem, .9)}')
for f in (1, 2):
    print(f'0 -> {f}')
    v = [x for x in out if x['f'] == f]
    show('all', v)
    for lo_, hi_ in ((0, 16), (17, 32), (33, 63), (64, 999)):
        w = [x for x in v if lo_ <= x['n'] <= hi_]
        if w: show(f'cmds {lo_}-{hi_}', w)
