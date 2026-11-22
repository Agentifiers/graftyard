"""Load curve per host, no leader clock in any duration. 15 s buckets of the near follower's clock: leader inputs/s
(count of Entered rows, bucketed by the leader's stale stamp, fine at 15 s), entries/s, agent commands cut/s, and the
leader excess per cycle = follower entry of k+1 - its answer to k - idle wire RT (p50, p90)."""
import os, sys; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import *
AG = 3  # AdmitClass index of Agent in Cut batch packing is found below by the largest class
for tag in sys.argv[1:]:
    a, b, rows = load(tag, sites=(0, 1), pad=30000); L = rows[0]
    ent = {I(r[3]): r[0] for r in rows[1] if r[1] == 25 and I(r[4]) % 64 == 1 and r[2] == 0}
    left = {I(r[3]): r[0] for r in rows[1] if r[1] == 24 and r[2] == 0}
    ans = {I(r[3]): r[0] for r in L if r[1] == 21 and r[2] == 1}
    idle = [ans[I(r[3])] - r[0] for r in L if r[1] == 23 and r[2] == 1 and r[0] < a - 2000 and I(r[3]) in ans]
    rt = pc(idle, .5)
    seen, E = set(), []
    for i, r in enumerate(L):
        if r[1] == 12 and r[2] == 1 and I(r[4]) not in seen:
            seen.add(I(r[4]))
            for j in range(i + 1, min(len(L), i + 400)):
                if L[j][1] == 23 and L[j][2] == 1: E.append(I(L[j][3])); break
    B = collections.defaultdict(lambda: [0, 0, 0, [], 0])
    for r in L:
        k = (r[0] - a) // 15000
        if r[1] == 25: B[k][0] += 1
        if r[1] == 29:
            bt = I(r[3]); B[k][2] += sum((bt >> (12 * i)) & 0xfff for i in range(4))
        if r[1] == 8 and I(r[5]) >= 20: B[k][4] += 1
    for n in range(len(E) - 1):
        c, c2 = E[n], E[n + 1]
        if c in left and c2 in ent and c in ent:
            g = ent[c2] - left[c] - rt
            k = (ent[c] - a) // 15000
            B[k][1] += 1
            if g < 6000: B[k][3].append(g)
    print(f'{tag} {COLO[tag]} idle RT {rt}: t s | inputs/s | entries/s | cmds cut/s | excess p50/p90 | stalls')
    print('   ' + ' | '.join(f'{15 * k}: {v[0] / 15:.0f}, {v[1] / 15:.2f}, {v[2] / 15:.0f}, {pc(v[3], .5)}/{pc(v[3], .9)}, {v[4]}' for k, v in sorted(B.items()) if -1 <= k < RUNS[tag][1] // 15))
