"""Scratch: slow rounds: per round, both voter legs' same-clock parts. And which percentile the doc's 'p99 round' is."""
import bisect, collections, json, lzma, sys
from datetime import datetime
def ms(s): return int(datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp() * 1000)
def pc(xs, q):
    xs = sorted(xs); return round(xs[min(len(xs) - 1, int(q * len(xs)))]) if xs else None
path, a, doc = sys.argv[1], ms(sys.argv[2]), int(sys.argv[3]); lo, hi = a + 30000, a + 270000
rows, seen = collections.defaultdict(list), set()
for line in lzma.open(path, 'rt'):
    d = json.loads(line)
    if 'rows' not in d or 'learner' in d: continue
    for r in d['rows']:
        k = (d.get('site'),) + tuple(r)
        if k not in seen and a <= r[0] < a + 300000: seen.add(k); rows[d.get('site')].append(r)
L = sorted(rows[0], key=lambda r: r[0])
first, acks = {}, collections.defaultdict(list)
for r in L:
    if r[1] == 12 and r[2] < 192: first.setdefault(int(float(r[4])), (r[0], int(float(r[3]))))
    if r[1] == 14 and r[2] < 192 and int(float(r[4])) == 0: acks[int(float(r[3]))].append(r[0])
allr = sorted(min((x for x in acks.get(rnd, []) if x >= t), default=t) - t for i, (t, rnd) in first.items() if a <= t < a + 300000 and acks.get(rnd))
print(f'entry rounds over the whole 300 s n {len(allr)}: p50 {pc(allr, .5)} p90 {pc(allr, .9)} p95 {pc(allr, .95)} p99 {pc(allr, .99)}; the doc\'s "p99 round" {doc} is this distribution\'s p{100 * bisect.bisect_left(allr, doc) / len(allr):.0f}')
# every append round incl. empty/heartbeat (any AppendIssued->AppendEnded ms to voters)
every = sorted(int(float(r[5])) for r in L if r[1] == 14 and r[2] < 192 and a <= r[0] < a + 300000)
print(f'every voter AppendEnded ms (per call, with entries) n {len(every)}: p50 {pc(every, .5)} p99 {pc(every, .99)}')
sent = {}
for r in L:
    if r[1] == 23 and r[2] < 192: sent[(r[2], int(float(r[3])))] = r[0]
ans = {(r[2], int(float(r[3]))): r[0] for r in L if r[1] == 21 and r[2] < 192}
legs = collections.defaultdict(dict)   # entry append time -> follower -> parts
qts = sorted(t for t, _ in first.values())
for f in (1, 2):
    F = rows[f]
    FS = sorted((r[0] - int(float(r[5])), r[0]) for r in F if r[1] == 31); fb = [x[0] for x in FS]
    left = {int(float(r[3])): (r[0], int(float(r[4]))) for r in F if r[1] == 24 and r[2] == 0}
    opn = {int(float(r[3])): int(float(r[5])) for r in F if r[1] == 25 and int(float(r[4])) % 64 == 1}
    for (p, c), t in sent.items():
        if p != f or (p, c) not in ans or c not in left: continue
        j = bisect.bisect_right(qts, t) - 1
        if j < 0 or t - qts[j] > 5: continue
        ret, entry = left[c]
        k = bisect.bisect_left(fb, entry)
        fg = max(0, FS[k][1] - ret) if k < len(FS) and FS[k][0] <= ret + 300 else 0
        legs[qts[j]][f] = {'leg': ans[(p, c)] - t, 'h': ret - entry, 'fg': fg, 'open': opn.get(c, 0)}
R = []
for i, (t, rnd) in first.items():
    ack = min((x for x in acks.get(rnd, []) if x >= t), default=None)
    if ack is None or not lo <= t < hi or len(legs.get(t, {})) < 2: continue
    w = min(legs[t].values(), key=lambda x: x['leg'])
    R.append((ack - t, w, legs[t]))
R.sort(key=lambda x: x[0]); n = len(R)
for name, sl in (('fastest half', R[:n // 2]), ('p50-p90', R[n // 2:int(.9 * n)]), ('slowest 10%', R[int(.9 * n):])):
    g = lambda k: pc([x[1][k] for x in sl], .5)
    both = [min(v['h'] + v['fg'] for v in x[2].values()) for x in sl]
    print(f'  {name:12} n {len(sl):3} round p50 {pc([x[0] for x in sl], .5):4} | winning leg: follower handler p50 {g("h")}, its confirm after return {g("fg")}, open at entry {g("open")} | '
          f'faster follower\'s handler+confirm p50 {pc(both, .5)} p90 {pc(both, .9)} | round minus that p50 {pc([x[0] - b for x, b in zip(sl, both)], .5)}')
