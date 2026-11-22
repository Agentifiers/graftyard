"""What the leader's excess follows, on follower clocks only. Per cycle k (near voter f=1): excess = follower entry of
k+1 - its answer to k - idle wire RT. x1 = requests entering the leader's handler per second over the real cycle
(ring count of Entered between the two CallSent / follower-clock cycle). x2 = encoded bytes of entry k+1 (the one the
leader had to build and send), KB. Median excess per cell, pooled by host class; whole run after 15 s."""
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
G = {'EWR (sabA, blB)': ['sabA', 'blB'], 'ORD (crA)': ['crA'], 'IAD (blA, chA, blC2)': ['blA', 'chA', 'blC2'], 'ATL (ncA)': ['ncA']}
RB = ((0, 200), (200, 300), (300, 400), (400, 10 ** 6)); KB = ((0, 60), (60, 130), (130, 10 ** 6))
for name, tags in G.items():
    V = []
    for tag in tags:
        a, L = raw(tag, 0); _, F = raw(tag, 1)
        ent = {I(r[3]): r[0] for r in F if r[1] == 25 and I(r[4]) % 64 == 1 and r[2] == 0}
        left = {I(r[3]): r[0] for r in F if r[1] == 24 and r[2] == 0}
        ans = {I(r[3]): r[0] for r in L if r[1] == 21 and r[2] == 1}
        rt = pc([ans[I(r[3])] - r[0] for r in L if r[1] == 23 and r[2] == 1 and r[0] < a - 2000 and I(r[3]) in ans], .5)
        ne = [0]
        for r in L: ne.append(ne[-1] + (r[1] == 25))
        seen, E = set(), []
        for i, r in enumerate(L):
            if r[1] == 12 and r[2] == 1 and I(r[4]) not in seen:
                seen.add(I(r[4]))
                for j in range(i + 1, min(len(L), i + 400)):
                    if L[j][1] == 23 and L[j][2] == 1: E.append((I(L[j][3]), j, I(r[5]))); break
        for n in range(len(E) - 1):
            (c, p, _), (c2, p2, by) = E[n], E[n + 1]
            if c in left and c2 in ent and c in ent and ent[c] >= a + 15000:
                cyc = ent[c2] - ent[c]; g = ent[c2] - left[c] - rt
                if 0 < cyc < 6000: V.append((g, 1000 * (ne[p2] - ne[p]) / cyc, by / 1000))
    print(f'{name}: cycles {len(V)}; median excess ms (n) by leader inputs/s (rows) and next entry KB (cols <60 / 60-130 / >=130), then all sizes')
    for lo, hi in RB:
        row = []
        for kl, kh in KB + ((0, 10 ** 6),):
            w = [v[0] for v in V if lo <= v[1] < hi and kl <= v[2] < kh]
            row.append(f'{pc(w, .5)} ({len(w)})' if len(w) >= 8 else f'- ({len(w)})')
        print(f'   {lo:3}-{hi if hi < 10 ** 6 else "":3}/s | ' + ' | '.join(row))
    row = []
    for kl, kh in KB:
        w = [v[0] for v in V if kl <= v[2] < kh]; row.append(f'{pc(w, .5)} ({len(w)})')
    print('   all rates | ' + ' | '.join(row))
