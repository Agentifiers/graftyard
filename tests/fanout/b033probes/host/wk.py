"""Is an entry's AppendEntries held until the leader's own transaction confirms? No leader clock: the real length of a
confirm is floored by the 100 ms ticker wakes counted inside it (Sync row `peer`; N wakes => at least (N-1) x 100 ms
real, loop running), order is ring order, and times are the near follower's clock.
Per entry k: own Sync = the first Sync after k's Flushed row with the same statement count. By wakes inside it:
  answered-first = the near voter's answer to k was stepped (AnswerArrived) before that Sync row.
  G = follower entry of k - follower's answer to k-1 - idle wire RT (what the leader took before k arrived)
  N = the same for k+1 (after k).
Held behind its own confirm: answered-first ~0% and G >= the wake floor in the long bins. Not held: answered-first high
(a voter round trip is ~100 ms) and G flat."""
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
            if k not in seen and a - 30000 <= r[0] < b: seen.add(k); out.append(r)
    return a, out
G = {'EWR (sabA, blB)': ['sabA', 'blB'], 'ORD, IAD, ATL (crA, blA, chA, blC2, ncA)': ['crA', 'blA', 'chA', 'blC2', 'ncA']}
for name, tags in G.items():
    V = []
    for tag in tags:
        a, L = raw(tag, 0); _, F = raw(tag, 1)
        ent = {I(r[3]): r[0] for r in F if r[1] == 25 and I(r[4]) % 64 == 1 and r[2] == 0}
        left = {I(r[3]): r[0] for r in F if r[1] == 24 and r[2] == 0}
        ansp = {I(r[3]): i for i, r in enumerate(L) if r[1] == 21 and r[2] == 1}
        ans = {I(r[3]): r[0] for r in L if r[1] == 21 and r[2] == 1}
        rt = pc([ans[I(r[3])] - r[0] for r in L if r[1] == 23 and r[2] == 1 and r[0] < a - 2000 and I(r[3]) in ans], .5)
        seen, E = set(), []
        for i, r in enumerate(L):
            if r[1] == 12 and r[2] == 1 and I(r[4]) not in seen:
                seen.add(I(r[4])); cs = fl = sy = call = None
                for j in range(i + 1, min(len(L), i + 4000)):
                    x = L[j]
                    if cs is None and x[1] == 23 and x[2] == 1: cs = j; call = I(x[3])
                    elif cs is not None and fl is None and x[1] == 16: fl = j; n = I(x[4])
                    elif fl is not None and x[1] == 31 and (I(x[4]) & 0xfffff) == n: sy = j; break
                E.append((call, cs, sy, r[0]))
        for k in range(1, len(E) - 1):
            call, cs, sy, t = E[k]; pc_, nx = E[k - 1][0], E[k + 1][0]
            if None in (call, sy, pc_, nx) or t < a + 60000: continue
            if call not in ent or call not in left or pc_ not in left or nx not in ent or call not in ansp: continue
            g = ent[call] - left[pc_] - rt; n_ = ent[nx] - left[call] - rt
            if g > 6000 or n_ > 6000: continue
            V.append((L[sy][2], ansp[call] < sy, g, n_))
    print(f'{name}: entries {len(V)}')
    print('   wakes inside own confirm (real floor) | n | answered-first | G p25/p50/p75 | N p25/p50/p75')
    for lo, hi, lab in ((0, 1, '0'), (1, 3, '1-2 (>=0-100 ms)'), (3, 5, '3-4 (>=200 ms)'), (5, 10, '5-9 (>=400 ms)'), (10, 999, '10+ (>=900 ms)')):
        w = [v for v in V if lo <= v[0] < hi]
        if not w: continue
        print(f'   {lab:18} | {len(w):4} | {sum(v[1] for v in w):3} ({100 * sum(v[1] for v in w) / len(w):3.0f}%) | {pc([v[2] for v in w], .25)}/{pc([v[2] for v in w], .5)}/{pc([v[2] for v in w], .75)} | {pc([v[3] for v in w], .25)}/{pc([v[3] for v in w], .5)}/{pc([v[3] for v in w], .75)}')
