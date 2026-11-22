"""The cycle on the followers' clocks, which are not stale (the loaded leader's Date.now() is 1-3 s behind; clamp.py).
Idle rows before load give each follower's one-way wire w_f (half the idle round trip) and its clock offset to the
leader, so follower stamps map to one real clock within about 20 ms. Per entry k, steady window:
  a1 = near voter's ack physically at the leader (L1_k + w1); a2 = far voter's (L2_k + w2)
  S' = next entry physically leaving the leader, if nothing held it: E1_{k+1} - w1 (cross-check: E2_{k+1} - w2)
  T1 = S' - a1: leader turnaround after the first voter ack.  T2 = S' - max(a1, a2): after BOTH voter acks, when the
  entry is committed without the leader's own copy. Whatever T2 holds is the leader taking events late or its next
  send being held; it is not waiting for a quorum.
  cycle = E1_{k+1} - E1_k. H1 = follower handling."""
import os, sys; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import *
print('run    colo | n | cycle p50/p90 | near handle p50 | wire RT near/far (idle) | T1 p50/p90 | T2 p50/p90 | T2 share of cycle (sum) | S\' near-vs-far disagreement p50/p90 | far ack after near p50')
for tag in sys.argv[1:]:
    a, b, rows = load(tag, sites=(0, 1, 2), pad=30000); L = rows[0]
    sent = {(r[2], I(r[3])): r[0] for r in L if r[1] == 23 and r[2] < 192}
    ans = {(r[2], I(r[3])): r[0] for r in L if r[1] == 21 and r[2] < 192}
    first = {}
    for r in L:
        if r[1] == 12 and r[2] < 192: first.setdefault(I(r[4]), r[0])
    idx = sorted(first, key=lambda i: first[i]); qts = [first[i] for i in idx]
    w, off, EL = {}, {}, {}
    for f in (1, 2):
        ent = {I(r[3]): r[0] for r in rows[f] if r[1] == 25 and I(r[4]) % 64 == 1 and r[2] == 0}
        left = {I(r[3]): r[0] for r in rows[f] if r[1] == 24 and r[2] == 0}
        idle = [(ent[c] - t, ans[(p, c)] - left[c], left[c] - ent[c]) for (p, c), t in sent.items() if p == f and t < a - 2000 and c in ent and c in left and (p, c) in ans]
        o = pc([x[0] for x in idle], .5); bk = pc([x[1] for x in idle], .5)
        w[f] = (o + bk) / 2; off[f] = (o - bk) / 2   # follower clock = leader clock + off
        EL[f] = {}
        for r in L:
            if r[1] != 23 or r[2] != f: continue
            c = I(r[3]); j = bisect.bisect_right(qts, r[0]) - 1
            if j < 0 or r[0] - qts[j] > 5 or j in EL[f] or c not in ent or c not in left: continue
            EL[f][j] = (ent[c] - off[f], left[c] - off[f])
    V = []
    for n in range(len(idx) - 1):
        if qts[n] < a + 90000 or qts[n] >= b: continue
        if not all(n in EL[f] and n + 1 in EL[f] for f in (1, 2)): continue
        a1 = EL[1][n][1] + w[1]; a2 = EL[2][n][1] + w[2]
        s1 = EL[1][n + 1][0] - w[1]; s2 = EL[2][n + 1][0] - w[2]
        cyc = EL[1][n + 1][0] - EL[1][n][0]
        if cyc > 6000: continue
        s = min(s1, s2)
        V.append((cyc, EL[1][n][1] - EL[1][n][0], s - a1, s - max(a1, a2), s1 - s2, a2 - a1))
    if not V: continue
    q = lambda i: f'{pc([v[i] for v in V], .5)}/{pc([v[i] for v in V], .9)}'
    print(f'{tag:6} {COLO[tag]} | {len(V)} | {q(0)} | {pc([v[1] for v in V], .5)} | {2 * w[1]:.0f}/{2 * w[2]:.0f} | {q(2)} | {q(3)} | {100 * sum(max(0, v[3]) for v in V) / sum(v[0] for v in V):.0f}% | {pc([abs(v[4]) for v in V], .5)}/{pc([abs(v[4]) for v in V], .9)} | {pc([v[5] for v in V], .5)}')
