"""Scratch: virtual Human probe on a tsave trace. For each instant t in the load window, a Human command admitted at t
joins the first cut after t (Humans are taken first, byte-bounded only), and commits when that cut's entry applies.
Splits p_i - t into freeze wait (t -> cut), held (cut -> first AppendQueued), round (-> first ok voter ack), apply.
    python3 probe.py TRACE.jsonl.xz SITE FROM_ISO SECONDS
"""
import bisect, collections, json, lzma, sys
from datetime import datetime


def ms(s):
    return int(datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp() * 1000)


def pc(xs, q):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(q * len(xs)))] if xs else None


path, s, a = sys.argv[1], int(sys.argv[2]), ms(sys.argv[3])
b = a + 1000 * int(sys.argv[4])
rows, seen = [], set()
for line in lzma.open(path, 'rt'):
    d = json.loads(line)
    if 'rows' not in d or 'learner' in d or d.get('site') != s:
        continue
    for r in d['rows']:
        k = tuple(r)
        if k not in seen and a <= r[0] < b:
            seen.add(k)
            rows.append(r)
R = sorted(rows, key=lambda r: r[0])
kinds = collections.Counter(r[1] for r in R)
print('kinds', dict(sorted(kinds.items())))
app, first, acks = {}, {}, collections.defaultdict(list)
for pos, r in enumerate(R):
    if r[1] == 4:
        app.setdefault(int(float(r[4])), r[0])
    if r[1] == 12 and r[2] < 192:
        first.setdefault(int(float(r[4])), (r[0], int(float(r[3])), pos))
    if r[1] == 14 and r[2] < 192 and int(float(r[4])) == 0:
        acks[int(float(r[3]))].append(r[0])
idxs = sorted(first, key=lambda i: first[i][2])
apos = [first[i][2] for i in idxs]
cut = {}
for pos, r in enumerate(R):
    if r[1] != 29:
        continue
    k = bisect.bisect_left(apos, pos) - 1
    if k < 0:
        continue
    n = idxs[k] + (r[2] // 16) % 4 + 1
    batch, left = int(float(r[3])), int(float(r[4]))
    cut.setdefault(n, {'at': r[0], 'end': r[2] % 16, 'held': (r[2] // 16) % 4, 'inflight': r[2] >= 64,
                       'taken': [(batch >> (12 * i)) & 0xfff for i in range(4)],
                       'left': [(left >> (13 * i)) & 0x1fff for i in range(4)]})
E = []
for i in idxs:
    if i not in cut:
        continue
    t, rnd, _ = first[i]
    ack = min((x for x in acks.get(rnd, []) if x >= t), default=None)
    if ack is None:
        continue
    E.append({'i': i, 'cut': cut[i]['at'], 'q': t, 'ack': ack, 'ap': max(app.get(i, ack), ack), **cut[i]})
E.sort(key=lambda e: e['cut'])
print(f'entries {len(E)} over {(E[-1]["cut"] - E[0]["cut"]) / 1000:.0f} s; human left>0 after cut: '
      f'{sum(e["left"][0] > 0 for e in E)}; mergeq left>0: {sum(e["left"][1] > 0 for e in E)}')
print('human taken total', sum(e['taken'][0] for e in E), 'agent', sum(e['taken'][3] for e in E), 'bot',
      sum(e['taken'][2] for e in E), 'mergeq', sum(e['taken'][1] for e in E))
print('cut held dist', dict(collections.Counter(e['held'] for e in E)), 'inflight',
      sum(e['inflight'] for e in E))
rounds = [e['ack'] - e['q'] for e in E]
print(f'round p50 {pc(rounds, .5)} p90 {pc(rounds, .9)} p99 {pc(rounds, .99)}; held(cut->queued) p50 '
      f'{pc([e["q"] - e["cut"] for e in E], .5)} p90 {pc([e["q"] - e["cut"] for e in E], .9)} p99 '
      f'{pc([e["q"] - e["cut"] for e in E], .99)}; apply p50 {pc([e["ap"] - e["ack"] for e in E], .5)} p99 '
      f'{pc([e["ap"] - e["ack"] for e in E], .99)}')
gaps = [E[j]['cut'] - E[j - 1]['cut'] for j in range(1, len(E))]
print(f'cut-to-cut gap p50 {pc(gaps, .5)} p90 {pc(gaps, .9)} p99 {pc(gaps, .99)} max {max(gaps)}')
# lag-1 autocorrelation of rounds
m = sum(rounds) / len(rounds)
var = sum((x - m) ** 2 for x in rounds)
ac = sum((rounds[j] - m) * (rounds[j - 1] - m) for j in range(1, len(rounds))) / var
print(f'round lag-1 autocorrelation {ac:.2f}')
# round vs commands taken
bins = collections.defaultdict(list)
for e in E:
    n = sum(e['taken'])
    bins['<=16' if n <= 16 else '17-32' if n <= 32 else '33-63' if n <= 63 else '64-80' if n <= 80 else '>80'].append(
        e['ack'] - e['q'])
for k in ('<=16', '17-32', '33-63', '64-80', '>80'):
    if bins[k]:
        print(f'  cmds {k}: n {len(bins[k])} round p50 {pc(bins[k], .5)} p90 {pc(bins[k], .9)}')
# virtual probe: every 5 ms
P = []
cuts = [e['cut'] for e in E]
for t in range(E[0]['cut'], E[-1]['cut'], 5):
    j = bisect.bisect_left(cuts, t)
    e = E[j]
    P.append((e['ap'] - t, e['cut'] - t, e['q'] - e['cut'], e['ack'] - e['q'], e['ap'] - e['ack'], e['held']))
tot = [p[0] for p in P]
print(f'virtual human (time-uniform arrival): p50 {pc(tot, .5)} p90 {pc(tot, .9)} p99 {pc(tot, .99)} '
      f'share<=1000ms {100 * sum(x <= 1000 for x in tot) / len(tot):.1f}%')
for lo, hi, name in ((.45, .55, 'median band'), (.985, .995, 'p99 band')):
    xs = sorted(P)[int(lo * len(P)):int(hi * len(P))]
    f = lambda k: round(sum(x[k] for x in xs) / len(xs))
    print(f'  {name}: total {f(0)} = freeze wait {f(1)} + held behind in-flight {f(2)} + own round {f(3)} + apply '
          f'{f(4)}; mean batches held ahead {sum(x[5] for x in xs) / len(xs):.2f}')
# human-weighted: each cut's humans arrive uniformly in (prev cut, cut]
W = []
for j in range(1, len(E)):
    e, h = E[j], E[j]['taken'][0]
    span = e['cut'] - E[j - 1]['cut']
    for k in range(h):
        t = e['cut'] - span * (k + .5) / h
        W.append(e['ap'] - t)
if W:
    print(f'human-weighted (taken counts, uniform within gap): n {len(W)} p50 {pc(W, .5):.0f} p90 {pc(W, .9):.0f} '
          f'p99 {pc(W, .99):.0f}')
# counterfactuals on the virtual probe
def cf(name, fn):
    xs = [fn(p) for p in P]
    print(f'  what-if {name}: p50 {pc(xs, .5):.0f} p99 {pc(xs, .99):.0f}')
cf('no held-behind (late join newest frozen batch)', lambda p: p[0] - p[2])
r50 = pc(rounds, .5)
cf('rounds capped at 300 ms (crude: scale all parts by min(1,300/own round))', lambda p: p[0] * min(1, 300 / max(1, p[3])))
