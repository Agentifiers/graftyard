"""sync-ab's three arms on follower clocks: does removing leader inputs shorten the leader's excess per cycle?"""
import os, sys; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import common
from common import *
common.RUNS.update({'sabC': ('2026-11-21T11:48:41Z', 300), 'sabB2': ('2026-11-21T12:09:54Z', 300)})
print('arm | inputs/s (leader, 90-300 s) | cycles | cycle mean / p50 / p90 (near follower clock) | idle wire RT | leader excess p50/p90 | follower handle p50/p90 | entries/s | agent cmds per entry mean | x = agent/s')
for tag in ('sabA', 'sabC', 'sabB2'):
    a, b, rows = load(tag, sites=(0, 1), pad=30000); L = rows[0]
    ent = {I(r[3]): r[0] for r in rows[1] if r[1] == 25 and I(r[4]) % 64 == 1 and r[2] == 0}
    left = {I(r[3]): r[0] for r in rows[1] if r[1] == 24 and r[2] == 0}
    ans = {I(r[3]): r[0] for r in L if r[1] == 21 and r[2] == 1}
    rt = pc([ans[I(r[3])] - r[0] for r in L if r[1] == 23 and r[2] == 1 and r[0] < a - 2000 and I(r[3]) in ans], .5)
    seen, E = set(), []
    for i, r in enumerate(L):
        if r[1] == 12 and r[2] == 1 and I(r[4]) not in seen:
            seen.add(I(r[4]))
            for j in range(i + 1, min(len(L), i + 400)):
                if L[j][1] == 23 and L[j][2] == 1: E.append(I(L[j][3])); break
    V = []
    for n in range(len(E) - 1):
        c, c2 = E[n], E[n + 1]
        if c in left and c2 in ent and c in ent and a + 90000 <= ent[c] < b:
            cyc = ent[c2] - ent[c]
            if cyc < 6000: V.append((cyc, ent[c2] - left[c] - rt, left[c] - ent[c]))
    inp = sum(1 for r in L if r[1] == 25 and a + 90000 <= r[0] < b) / 210
    cuts = [sum((I(r[3]) >> (12 * i)) & 0xfff for i in range(4)) for r in L if r[1] == 29 and a + 90000 <= r[0] < b]
    cyc = sum(v[0] for v in V) / len(V)
    print(f'{tag:6} | {inp:.0f} | {len(V)} | {cyc:.0f} / {pc([v[0] for v in V], .5)} / {pc([v[0] for v in V], .9)} | {rt} | {pc([v[1] for v in V], .5)}/{pc([v[1] for v in V], .9)} | {pc([v[2] for v in V], .5)}/{pc([v[2] for v in V], .9)} | {1000 / cyc:.2f} | {sum(cuts) / max(1, len(cuts)):.1f} (cuts {len(cuts) / 210:.2f}/s) | {sum(cuts) / 210:.1f}')
