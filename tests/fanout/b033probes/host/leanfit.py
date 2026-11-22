"""On the two arms where the leader's clock is close to fresh (sabB2 168 inputs/s, sabC 261): per entry, near voter,
  pre  = follower Entered - leader CallSent - offset - idle one-way: time from the START of the turn that sends the
         entry (the isolate's clock does not move inside a turn) to the bytes reaching the follower, less the wire.
         That is the turn's own CPU (apply k-1, decide k, encode, queue) plus anything holding the send.
  post = leader AnswerArrived - follower CallLeft + offset - idle one-way: the answer waiting to be taken.
by commands in the entry and by encoded KB."""
import os, sys; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import common
from common import *
common.RUNS.update({'sabC': ('2026-11-21T11:48:41Z', 300), 'sabB2': ('2026-11-21T12:09:54Z', 300)})
def fit(V, i, j):
    n = len(V); mx = sum(v[i] for v in V) / n; my = sum(v[j] for v in V) / n
    sxx = sum((v[i] - mx) ** 2 for v in V); sxy = sum((v[i] - mx) * (v[j] - my) for v in V)
    b = sxy / sxx; return f'{my - b * mx:.0f} + {b:.2f}/cmd'
for tag in sys.argv[1:]:
    a, b, rows = load(tag, sites=(0, 1), pad=30000); L = rows[0]
    sent = {I(r[3]): (r[0], i) for i, r in enumerate(L) if r[1] == 23 and r[2] == 1}
    ans = {I(r[3]): r[0] for r in L if r[1] == 21 and r[2] == 1}
    ent = {I(r[3]): r[0] for r in rows[1] if r[1] == 25 and I(r[4]) % 64 == 1 and r[2] == 0}
    left = {I(r[3]): r[0] for r in rows[1] if r[1] == 24 and r[2] == 0}
    idle = [(ent[c] - t, ans[c] - left[c]) for c, (t, _) in sent.items() if t < a - 2000 and c in ent and c in left and c in ans]
    o = pc([x[0] for x in idle], .5); bk = pc([x[1] for x in idle], .5); w = (o + bk) / 2; off = (o - bk) / 2
    nc = [0]
    for r in L: nc.append(nc[-1] + (sum((I(r[3]) >> (12 * i)) & 0xfff for i in range(4)) if r[1] == 29 else 0))
    seen, E = set(), []
    for i, r in enumerate(L):
        if r[1] == 12 and r[2] == 1 and I(r[4]) not in seen:
            seen.add(I(r[4]))
            for j in range(i + 1, min(len(L), i + 400)):
                if L[j][1] == 23 and L[j][2] == 1: E.append((I(L[j][3]), i, I(r[5]) / 1000)); break
    V = []
    for n in range(1, len(E)):
        c, p, kb = E[n]; p0 = E[n - 1][1]
        if c not in ent or c not in left or c not in ans or not (a + 15000 <= sent[c][0] < b): continue
        cm = nc[p] - nc[p0]
        if cm <= 0: continue
        V.append((cm, ent[c] - sent[c][0] - off - w, ans[c] - left[c] + off - w, kb))
    print(f'{tag}: entries {len(V)}, idle one-way {w:.0f} ms')
    for lo, hi in ((1, 17), (17, 33), (33, 49), (49, 64), (64, 999)):
        x = [v for v in V if lo <= v[0] < hi]
        if len(x) >= 5: print(f'   cmds {lo:2}-{hi - 1:3} n {len(x):3} | KB p50 {pc([v[3] for v in x], .5):4} | pre p25/p50/p75 {pc([v[1] for v in x], .25)}/{pc([v[1] for v in x], .5)}/{pc([v[1] for v in x], .75)} | post p25/p50/p75 {pc([v[2] for v in x], .25)}/{pc([v[2] for v in x], .5)}/{pc([v[2] for v in x], .75)} | pre+post p50 {pc([v[1] + v[2] for v in x], .5)}')
    print(f'   fit pre = {fit(V, 0, 1)}; post = {fit(V, 0, 2)}')
