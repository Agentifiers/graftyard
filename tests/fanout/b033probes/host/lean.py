"""sabB2 and sabC: the leader's clock is not stale there (stale.py), so its own stamps can split a cycle. Clock offset
and one-way wire from the idle rows. Per entry k, near voter (1) and far (2), 90-300 s:
  out = follower Entered - leader CallSent - offset   (real way out: wire + anything holding the request)
  back = leader AnswerArrived - follower CallLeft + offset
  turn = next entry's CallSent - this entry's first-voter AnswerArrived (leader clock)
  own = this entry's own Sync resolve - CallSent
Also where the commit point came from."""
import os, sys; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import common
from common import *
common.RUNS.update({'sabC': ('2026-11-21T11:48:41Z', 300), 'sabB2': ('2026-11-21T12:09:54Z', 300)})
print('run | n | idle one-way near/far | cycle p50/p90 | near: out p50/p90, handle p50/p90, back p50/p90 | far: out p50/p90, back p50/p90 | turn (near ack -> next send) p10/p50/p90 | own confirm after send p50/p90 | out-idle>=100 ms share near, far, both together')
for tag in sys.argv[1:]:
    a, b, rows = load(tag, sites=(0, 1, 2), pad=30000); L = rows[0]
    sent = {(r[2], I(r[3])): r[0] for r in L if r[1] == 23 and r[2] < 192}
    ans = {(r[2], I(r[3])): r[0] for r in L if r[1] == 21 and r[2] < 192}
    first = {}
    for r in L:
        if r[1] == 12 and r[2] < 192: first.setdefault(I(r[4]), r[0])
    idx = sorted(first, key=lambda i: first[i]); qts = [first[i] for i in idx]
    S = sorted((r[0] - I(r[5]), r[0]) for r in L if r[1] == 31); sb = [x[0] for x in S]
    D = {}
    w = {}
    for f in (1, 2):
        ent = {I(r[3]): r[0] for r in rows[f] if r[1] == 25 and I(r[4]) % 64 == 1 and r[2] == 0}
        left = {I(r[3]): r[0] for r in rows[f] if r[1] == 24 and r[2] == 0}
        idle = [(ent[c] - t, ans[(p, c)] - left[c]) for (p, c), t in sent.items() if p == f and t < a - 2000 and c in ent and c in left and (p, c) in ans]
        o = pc([x[0] for x in idle], .5); bk = pc([x[1] for x in idle], .5); w[f] = (o + bk) / 2; off = (o - bk) / 2
        D[f] = {}
        for (p, c), t in sent.items():
            if p != f or c not in ent or c not in left or (p, c) not in ans: continue
            j = bisect.bisect_right(qts, t) - 1
            if j < 0 or t - qts[j] > 5 or j in D[f]: continue
            D[f][j] = (t, ent[c] - t - off, left[c] - ent[c], ans[(p, c)] - left[c] + off, ans[(p, c)])
    V = []
    for n in range(len(idx) - 1):
        if not (a + 90000 <= qts[n] < b) or n not in D[1] or n not in D[2] or n + 1 not in D[1]: continue
        d1, d2 = D[1][n], D[2][n]
        k = bisect.bisect_left(sb, qts[n])
        own = S[k][1] - qts[n] if k < len(S) and S[k][0] - qts[n] < 300 else None
        cyc = D[1][n + 1][0] - d1[0]
        if cyc > 6000: continue
        V.append((cyc, d1[1], d1[2], d1[3], d2[1], d2[3], D[1][n + 1][0] - min(d1[4], d2[4]), own))
    q = lambda i, v=None: f'{pc([x[i] for x in V if x[i] is not None], .5)}/{pc([x[i] for x in V if x[i] is not None], .9)}'
    n1 = sum(v[1] - w[1] >= 100 for v in V); n2 = sum(v[4] - w[2] >= 100 for v in V); nb = sum(v[1] - w[1] >= 100 and v[4] - w[2] >= 100 for v in V)
    print(f'{tag:6} | {len(V)} | {w[1]:.0f}/{w[2]:.0f} | {q(0)} | {q(1)}, {q(2)}, {q(3)} | {q(4)}, {q(5)} | {pc([v[6] for v in V], .1)}/{pc([v[6] for v in V], .5)}/{pc([v[6] for v in V], .9)} | {q(7)} | {100 * n1 / len(V):.0f}%, {100 * n2 / len(V):.0f}%, {100 * nb / len(V):.0f}%')
