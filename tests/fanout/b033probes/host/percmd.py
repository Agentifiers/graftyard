"""Is the leader's time per cycle proportional to the commands in the entry? Follower clock only. Per cycle k -> k+1
(near voter): cycle = follower entry of k+1 - follower entry of k; excess = cycle - idle wire RT - follower handling.
n = commands cut for entry k+1 (sum of Cut rows between the two appends, ring order), which the ack turn decides,
encodes and sends, having applied entry k. Whole run from 15 s (the ramp gives the spread in n).
Binned medians, then least squares excess = a + b*n, and the same within two input-rate bands."""
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
def fit(V):
    n = len(V)
    if n < 20: return None
    mx = sum(v[0] for v in V) / n; my = sum(v[1] for v in V) / n
    sxx = sum((v[0] - mx) ** 2 for v in V); sxy = sum((v[0] - mx) * (v[1] - my) for v in V)
    if sxx == 0: return None
    b = sxy / sxx; return my - b * mx, b
BINS = ((1, 17), (17, 33), (33, 49), (49, 64), (64, 10 ** 6))
print('run    colo | median excess ms (n cycles) by commands in the entry: 1-16 | 17-32 | 33-48 | 49-63 | 64+ | fit excess = a + b*cmds (ms, ms/cmd) | same at <250 inputs/s | at >=250 | implied agent ceiling 1000/(b + (a+RT)/64)')
for tag in sys.argv[1:]:
    a, L = raw(tag, 0); _, F = raw(tag, 1)
    ent = {I(r[3]): r[0] for r in F if r[1] == 25 and I(r[4]) % 64 == 1 and r[2] == 0}
    left = {I(r[3]): r[0] for r in F if r[1] == 24 and r[2] == 0}
    ans = {I(r[3]): r[0] for r in L if r[1] == 21 and r[2] == 1}
    rt = pc([ans[I(r[3])] - r[0] for r in L if r[1] == 23 and r[2] == 1 and r[0] < a - 2000 and I(r[3]) in ans], .5)
    ne = [0]; nc = [0]
    for r in L:
        ne.append(ne[-1] + (r[1] == 25))
        nc.append(nc[-1] + (sum((I(r[3]) >> (12 * i)) & 0xfff for i in range(4)) if r[1] == 29 else 0))
    seen, E = set(), []
    for i, r in enumerate(L):
        if r[1] == 12 and r[2] == 1 and I(r[4]) not in seen:
            seen.add(I(r[4]))
            for j in range(i + 1, min(len(L), i + 400)):
                if L[j][1] == 23 and L[j][2] == 1: E.append((I(L[j][3]), i)); break
    V = []
    for n in range(len(E) - 1):
        (c, p), (c2, p2) = E[n], E[n + 1]
        if not (c in left and c2 in ent and c in ent and ent[c] >= a + 15000): continue
        cyc = ent[c2] - ent[c]
        if not 0 < cyc < 6000: continue
        cm = nc[p2] - nc[p]
        if cm <= 0: continue
        V.append((cm, cyc - rt - (left[c] - ent[c]), 1000 * (ne[p2] - ne[p]) / cyc))
    cells = []
    for lo, hi in BINS:
        w = [v[1] for v in V if lo <= v[0] < hi]; cells.append(f'{pc(w, .5)} ({len(w)})' if len(w) >= 5 else f'- ({len(w)})')
    f = fit(V); fl = fit([v for v in V if v[2] < 250]); fh = fit([v for v in V if v[2] >= 250])
    s = lambda f: f'{f[0]:.0f} + {f[1]:.1f}/cmd' if f else '-'
    ceil = 1000 / (f[1] + (f[0] + rt) / 64) if f and f[1] + (f[0] + rt) / 64 > 0 else None
    print(f'{tag:6} {COLO[tag]} | ' + ' | '.join(cells) + f' | {s(f)} | {s(fl)} | {s(fh)} | {ceil and round(ceil)}')
