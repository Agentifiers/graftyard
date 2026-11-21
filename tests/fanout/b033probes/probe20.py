"""Scratch: does the leader's own storage confirm bound its append round? For each entry: W = how long after its first
AppendQueued the last leader sync() that had begun by then (same stamp included: the entry's own write) resolves;
R = first AppendQueued -> first ok voter AppendEnded; A = that ack -> Applied. One clock (the leader's).
If the fetch or its answer waits on the confirm, R >= W whenever W is long. If the confirm only gates commit
(`own_durable`), R stays a normal leg and A = W - R absorbs the rest."""
import bisect, collections, json, lzma, sys
from datetime import datetime
def ms(s): return int(datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp() * 1000)
def pc(xs, q):
    xs = sorted(xs); return round(xs[min(len(xs) - 1, int(q * len(xs)))]) if xs else None
path, a = sys.argv[1], ms(sys.argv[2]); lo, hi = a + 30000, a + 270000
rows, seen = [], set()
for line in lzma.open(path, 'rt'):
    d = json.loads(line)
    if 'rows' not in d or 'learner' in d or d.get('site') != 0: continue
    for r in d['rows']:
        k = tuple(r)
        if k not in seen and a <= r[0] < a + 300000: seen.add(k); rows.append(r)
L = sorted(rows, key=lambda r: r[0])
app, first, acks = {}, {}, collections.defaultdict(list)
for r in L:
    if r[1] == 4: app.setdefault(int(float(r[4])), r[0])
    if r[1] == 12 and r[2] < 192: first.setdefault(int(float(r[4])), (r[0], int(float(r[3]))))
    if r[1] == 14 and r[2] < 192 and int(float(r[4])) == 0: acks[int(float(r[3]))].append(r[0])
S = sorted((r[0] - int(float(r[5])), r[0], int(float(r[4])) & 0xfffff) for r in L if r[1] == 31)
V = []
for i, (t, rnd) in first.items():
    ack = min((x for x in acks.get(rnd, []) if x >= t), default=None)
    if ack is None or not lo <= t < hi: continue
    W = max((e - t for b, e, _ in S if b <= t and e > t), default=0)
    V.append((W, ack - t, max(app.get(i, ack), ack) - ack))
print(f'entries {len(V)}; leader confirm pending/starting at the append, W: p50 {pc([v[0] for v in V], .5)} p90 {pc([v[0] for v in V], .9)} p99 {pc([v[0] for v in V], .99)}')
for lo_, hi_ in ((0, 60), (61, 150), (151, 400), (401, 99999)):
    w = [v for v in V if lo_ <= v[0] <= hi_]
    if not w: continue
    print(f'  W {lo_:3}-{hi_ if hi_ < 99999 else "":5} n {len(w):3}: round p50 {pc([v[1] for v in w], .5):5} p90 {pc([v[1] for v in w], .9):5} | rounds SHORTER than W: {sum(v[1] < v[0] for v in w):3} | '
          f'round minus W p10 {pc([v[1] - v[0] for v in w], .1):5} p50 {pc([v[1] - v[0] for v in w], .5):5} | ack -> applied p50 {pc([v[2] for v in w], .5)} p90 {pc([v[2] for v in w], .9)}')
slow = sorted(V, key=lambda v: v[1])[int(.9 * len(V)):]
print(f'  slowest 10% of rounds (n {len(slow)}, p50 {pc([v[1] for v in slow], .5)}): W >= 400 ms in {sum(v[0] >= 400 for v in slow)}, W >= 150 in {sum(v[0] >= 150 for v in slow)}; all rounds: W >= 400 in {sum(v[0] >= 400 for v in V)}')
cf = sorted(v[1] - max(0, v[0] - 100) if v[1] >= v[0] else v[1] for v in V)
r = sorted(v[1] for v in V)
print(f'  rounds now p50 {pc(r, .5)} p90 {pc(r, .9)} p99 {pc(r, .99)}; with W capped at 100 ms where the round outlasted it: p50 {pc(cf, .5)} p90 {pc(cf, .9)} p99 {pc(cf, .99)}')
