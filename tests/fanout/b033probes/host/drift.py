import os, sys; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import *
for tag in sys.argv[1:]:
    a, b, rows = load(tag, sites=(0, 1, 2), pad=3000); L = rows[0]
    sent = {(r[2], I(r[3])): r[0] for r in L if r[1] == 23 and r[2] < 192}
    ans = {(r[2], I(r[3])): r[0] for r in L if r[1] == 21 and r[2] < 192}
    for f in (1, 2):
        ent = {I(r[3]): r[0] for r in rows[f] if r[1] == 25 and I(r[4]) % 64 == 1 and r[2] == 0}
        left = {I(r[3]): r[0] for r in rows[f] if r[1] == 24 and r[2] == 0}
        buckets = collections.defaultdict(list)
        for (p, c), t in sent.items():
            if p == f and c in ent and c in left and (p, c) in ans:
                buckets[(t - a) // 30000].append((ent[c] - t, ans[(p, c)] - left[c], ans[(p, c)] - t, left[c] - ent[c]))
        print(tag, f, ' | '.join(f'{k*30}s n{len(v)} out {pc([x[0] for x in v],.1)}/{pc([x[0] for x in v],.5)}/{pc([x[0] for x in v],.9)} back {pc([x[1] for x in v],.5)} rt {pc([x[2] for x in v],.5)} h {pc([x[3] for x in v],.5)}' for k, v in sorted(buckets.items())))
