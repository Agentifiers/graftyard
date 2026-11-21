import collections, json, lzma, sys
from datetime import datetime
def ms(s): return int(datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp() * 1000)
def pc(xs, q):
    xs = sorted(xs); return xs[min(len(xs) - 1, int(q * len(xs)))] if xs else None
path, a = sys.argv[1], ms(sys.argv[2]); lo, hi = a + 30000, a + 270000
rows, seen = collections.defaultdict(list), set()
for line in lzma.open(path, 'rt'):
    d = json.loads(line)
    if 'rows' not in d or 'learner' in d: continue
    for r in d['rows']:
        k = (d.get('site'),) + tuple(r)
        if k not in seen and lo - 3000 <= r[0] < hi + 3000: seen.add(k); rows[d.get('site')].append(r)
L = rows[0]
sent = {(r[2], int(float(r[3]))): r[0] for r in L if r[1] == 23 and r[2] < 192 and lo <= r[0] < hi}
ans = {(r[2], int(float(r[3]))): (r[0], int(float(r[4]))) for r in L if r[1] == 21 and r[2] < 192}
tot = collections.Counter(); 
for f in (1, 2):
    left = {int(float(r[3])): (r[0], int(float(r[4]))) for r in rows[f] if r[1] == 24 and r[2] == 0}
    arr = {}
    v = []
    for (p, c), t in sent.items():
        if p != f or (p, c) not in ans or c not in left: continue
        leg = ans[(p, c)][0] - t
        # offset-free: follower-clock span from handler entry to return; leader-clock span sent->answer
        v.append((leg, left[c][0] - left[c][1], left[c][1] - t, ans[(p, c)][0] - left[c][0]))
    ex = [x for x in v if 995 <= x[0] <= 1005]
    print(f'0->{f}: calls {len(v)}; legs of 995..1005 ms: {len(ex)}; exactly 1000: {sum(1 for x in v if x[0] == 1000)}; legs 900..994: {sum(1 for x in v if 900 <= x[0] < 995)}; legs 1006..1100: {sum(1 for x in v if 1005 < x[0] <= 1100)}')
    for x in ex[:5]:
        print(f'    leg {x[0]}: follower handler {x[1]} ms; follower entry - leader send (cross-clock) {x[2]}; leader answer - follower return (cross-clock) {x[3]}')
    # skew proxy: median of (out - back)/2 for fast legs
    fast = [x for x in v if x[0] <= pc([y[0] for y in v], .2)]
    print(f'    fast fifth of legs (<= {pc([y[0] for y in v], .2)} ms): cross-clock out p50 {pc([x[2] for x in fast], .5)}, back p50 {pc([x[3] for x in fast], .5)}')
h = collections.Counter(min((ans[k][0] - t) // 50, 30) for k, t in sent.items() if k in ans)
print('leg histogram 50 ms bins 800..1250:', [(b * 50, h[b]) for b in range(16, 25)])
