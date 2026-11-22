"""Leader excess per cycle on ONE follower's clock: (follower handler entry of entry k+1) - (its answer to entry k
leaving) - the idle wire round trip between the two. Needs no leader clock. For the first leader (site 0) against
follower 1 and 2, and for the second leader after a transfer (ncA site 2, m64r33 site 1) against site 0 as follower."""
import os, sys; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import *
def allrows(tag):
    rows, seen = collections.defaultdict(list), set()
    for line in lzma.open(raw_path(tag), 'rt'):
        d = json.loads(line)
        if 'rows' not in d or 'learner' in d or d.get('site') not in (0, 1, 2): continue
        for r in d['rows']:
            k = (d['site'],) + tuple(r)
            if k not in seen: seen.add(k); rows[d['site']].append(r)
    for s in rows: rows[s].sort(key=lambda r: r[0])
    return rows
def excess(rows, ldr, f, t0, t1, rt):
    L = rows[ldr]
    ent = {I(r[3]): r[0] for r in rows[f] if r[1] == 25 and I(r[4]) % 64 == 1 and r[2] == ldr}
    left = {I(r[3]): r[0] for r in rows[f] if r[1] == 24 and r[2] == ldr}
    seen, E, i = set(), [], 0
    for i, r in enumerate(L):
        if r[1] == 12 and r[2] == f and I(r[4]) not in seen:
            seen.add(I(r[4]))
            for j in range(i + 1, min(len(L), i + 400)):
                if L[j][1] == 23 and L[j][2] == f: E.append(I(L[j][3])); break
    V = []
    for k in range(len(E) - 1):
        c, c2 = E[k], E[k + 1]
        if c in left and c2 in ent and c in ent and t0 <= ent[c] < t1:
            g = ent[c2] - left[c]
            if g < 6000: V.append((g - rt, ent[c2] - ent[c], left[c] - ent[c]))
    return V
ONLY = sys.argv[1:]
for tag, ldr, f, frm, to in (('sabA', 0, 1, 90, 300), ('blB', 0, 1, 90, 300), ('crA', 0, 1, 90, 300), ('blA', 0, 1, 90, 300), ('blC2', 0, 1, 90, 300), ('chA', 0, 1, 90, 300),
                             ('ncA', 0, 1, 90, 216), ('ncA', 0, 2, 90, 216), ('ncA', 2, 0, 225, 300), ('m64r33', 0, 1, 60, 144), ('m64r33', 0, 2, 60, 144), ('m64r33', 1, 0, 155, 300),
                             *[(q, 0, 1, 90, 300) for q in sys.argv[1:]]):
    if ONLY and tag not in ONLY: continue
    rows = allrows(tag); a = ms(RUNS[tag][0])
    other = f if ldr == 0 else ldr
    sent = {I(r[3]): r[0] for r in rows[0] if r[1] == 23 and r[2] == other and r[0] < a - 2000}
    ans = {I(r[3]): r[0] for r in rows[0] if r[1] == 21 and r[2] == other}
    rt = pc([ans[c] - t for c, t in sent.items() if c in ans], .5)
    V = excess(rows, ldr, f, a + frm * 1000, a + to * 1000, rt)
    if not V: print(tag, ldr, f, 'no rows'); continue
    cyc = sum(v[1] for v in V) / len(V)
    print(f'{tag:6} leader site {ldr} -> follower {f}, {frm}-{to} s | idle wire RT {rt} | cycles {len(V)} | cycle mean {cyc:.0f} p50 {pc([v[1] for v in V], .5)} | follower handle p50 {pc([v[2] for v in V], .5)} | leader excess p50/p90 {pc([v[0] for v in V], .5)}/{pc([v[0] for v in V], .9)} | excess share of cycle {100 * sum(v[0] for v in V) / sum(v[1] for v in V):.0f}% | entries/s {1000 / cyc:.2f}')
