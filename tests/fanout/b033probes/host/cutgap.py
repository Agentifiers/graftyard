"""Per cycle: was entry k+1's batch cut early (while k was in flight: Cut peer & 64) or only once k committed? And what
does each kind cost, on the near follower's clock (leader excess = follower entry of k+1 - its answer to k - idle RT)?
Also the gap Cut -> AppendQueued on the leader's stamps for at-commit cuts (deps read + decide), and commands cut."""
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
print('run    colo | cycles | early cut: n (share), excess p50/p90, cmds/entry | cut at commit: n, excess p50/p90, cmds/entry, Cut->AppendQueued ms p50/p90 (leader stamps), inputs entering in that gap p50 | cycles with 2+ cuts')
for tag in sys.argv[1:]:
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
                if L[j][1] == 23 and L[j][2] == 1: E.append((I(L[j][3]), i)); break
    early, late, multi = [], [], 0
    for n in range(len(E) - 1):
        (c, p), (c2, p2) = E[n], E[n + 1]
        if not (c in left and c2 in ent and c in ent and ent[c] >= a + 90000): continue
        g = ent[c2] - left[c] - rt
        if g > 6000: continue
        cuts = [(j, L[j]) for j in range(p, p2) if L[j][1] == 29]
        if not cuts: continue
        if len(cuts) > 1: multi += 1
        j, x = cuts[-1]
        n_cmd = sum((I(x[3]) >> (12 * i)) & 0xfff for i in range(4))
        if x[2] & 64: early.append((g, n_cmd))
        else: late.append((g, n_cmd, L[p2][0] - x[0], ne[p2] - ne[j]))
    q = lambda v, i: f'{pc([x[i] for x in v], .5)}/{pc([x[i] for x in v], .9)}'
    m = lambda v: sum(x[1] for x in v) / max(1, len(v))
    print(f'{tag:6} {COLO[tag]} | {len(early) + len(late)} | {len(early)} ({100 * len(early) / max(1, len(early) + len(late)):.0f}%), {q(early, 0)}, {m(early):.0f} | {len(late)}, {q(late, 0)}, {m(late):.0f}, {q(late, 2)}, {pc([x[3] for x in late], .5)} | {multi}')
