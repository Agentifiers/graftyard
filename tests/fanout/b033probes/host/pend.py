"""carried-6's gate, tested without the leader's clock. Ring order says whether entry k's own transaction was still
unconfirmed when entry k+1's fetch was issued (its Sync row comes after k+1's CallSent). The followers' clocks say how
long the leader took: G = follower handler entry of k+1 minus that follower's answer to k, minus the idle round trip
(so 0 = the leader turned the ack round at once). Gate: 'pending' cycles carry the excess; 'confirmed' cycles are short."""
import os, sys; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import *
def raw(tag, site):
    a = ms(RUNS[tag][0]); b = a + 1000 * RUNS[tag][1]
    out, seen = [], set()
    for line in lzma.open(raw_path(tag), 'rt'):
        d = json.loads(line)
        if 'rows' not in d or 'learner' in d or d.get('site') != site: continue
        for r in d['rows']:
            k = tuple(r)
            if k not in seen and a - 30000 <= r[0] < b + 3000: seen.add(k); out.append(r)
    return a, b, out
print('run    colo | cycles | k confirmed before k+1 left: n, leader excess p50/p90 (near voter clock) | k still unconfirmed: n (share), excess p50/p90 | far voter same: confirmed p50, unconfirmed p50')
for tag in sys.argv[1:]:
    a, b, L = raw(tag, 0)
    F = {f: raw(tag, f)[2] for f in (1, 2)}
    res = {}
    for f in (1, 2):
        ent = {I(r[3]): r[0] for r in F[f] if r[1] == 25 and I(r[4]) % 64 == 1 and r[2] == 0}
        left = {I(r[3]): r[0] for r in F[f] if r[1] == 24 and r[2] == 0}
        ans = {I(r[3]): r[0] for r in L if r[1] == 21 and r[2] == f}
        idle = [ans[I(r[3])] - r[0] for r in L if r[1] == 23 and r[2] == f and r[0] < a - 2000 and I(r[3]) in ans]
        rt = pc(idle, .5)
        # entries in ring order: AppendQueued(12) to f with a new last index, then its CallSent(23), Flushed(16), Sync(31)
        E = []; seen = set(); i = 0
        while i < len(L):
            r = L[i]
            if r[1] == 12 and r[2] == f and I(r[4]) not in seen:
                seen.add(I(r[4])); cs = fl = sy = None; call = None
                for j in range(i + 1, min(len(L), i + 3000)):
                    x = L[j]
                    if cs is None and x[1] == 23 and x[2] == f: cs = j; call = I(x[3])
                    elif cs is not None and fl is None and x[1] == 16: fl = j; n = I(x[4])
                    elif fl is not None and x[1] == 31 and (I(x[4]) & 0xfffff) == n: sy = j; break
                E.append((r[0], cs, sy, call))
            i += 1
        V = []
        for k in range(len(E) - 1):
            t, cs, sy, call = E[k]; t2, cs2, sy2, call2 = E[k + 1]
            if None in (cs, sy, call, cs2, call2) or t < a + 90000 or t >= b: continue
            if call not in left or call2 not in ent: continue
            g = ent[call2] - left[call] - rt
            if g > 6000: continue
            V.append((sy > cs2, g))
        res[f] = V
    c = [v[1] for v in res[1] if not v[0]]; p = [v[1] for v in res[1] if v[0]]
    c2 = [v[1] for v in res[2] if not v[0]]; p2 = [v[1] for v in res[2] if v[0]]
    print(f'{tag:6} {COLO[tag]} | {len(res[1])} | {len(c)}, {pc(c, .5)}/{pc(c, .9)} | {len(p)} ({100 * len(p) / max(1, len(res[1])):.0f}%), {pc(p, .5)}/{pc(p, .9)} | {pc(c2, .5)}, {pc(p2, .5)}')
