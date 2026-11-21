"""Scratch: (1) leg outside the handler, two-way by commands in the entry and follower open requests; (2) late-join
counterfactual for Humans; (3) where the freeze falls in the round in flight.
    python3 probe3.py TRACE.jsonl.xz SITE FROM_ISO SECONDS
"""
import bisect, collections, json, lzma, sys
from datetime import datetime


def ms(s):
    return int(datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp() * 1000)


def pc(xs, q):
    xs = sorted(xs)
    return round(xs[min(len(xs) - 1, int(q * len(xs)))]) if xs else None


path, s, a = sys.argv[1], int(sys.argv[2]), ms(sys.argv[3])
b = a + 1000 * int(sys.argv[4])
rows, seen = collections.defaultdict(list), set()
for line in lzma.open(path, 'rt'):
    d = json.loads(line)
    if 'rows' not in d or 'learner' in d:
        continue
    for r in d['rows']:
        k = (d.get('site'),) + tuple(r)
        if k not in seen and a - 3000 <= r[0] < b + 3000:
            seen.add(k)
            rows[d.get('site')].append(r)
R = sorted((r for r in rows[s] if a <= r[0] < b), key=lambda r: r[0])
app, first, acks = {}, {}, collections.defaultdict(list)
for pos, r in enumerate(R):
    if r[1] == 4:
        app.setdefault(int(float(r[4])), r[0])
    if r[1] == 12 and r[2] < 192:
        first.setdefault(int(float(r[4])), (r[0], int(float(r[3])), pos))
    if r[1] == 14 and r[2] < 192 and int(float(r[4])) == 0:
        acks[int(float(r[3]))].append((r[0], r[2]))
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
    batch = int(float(r[3]))
    cut.setdefault(n, {'cut': r[0], 'inflight': r[2] >= 64, 'taken': [(batch >> (12 * i)) & 0xfff for i in range(4)]})
E = []
for i in idxs:
    if i not in cut:
        continue
    t, rnd, _ = first[i]
    ack = min((x for x in acks.get(rnd, []) if x[0] >= t), default=None)
    if ack is None:
        continue
    E.append({'i': i, 'q': t, 'ack': ack[0], 'ap': max(app.get(i, ack[0]), ack[0]), **cut[i]})
E.sort(key=lambda e: e['cut'])
lo, hi = a + 30000, a + 270000
# (3) where the cut for k+1 falls relative to k's round
early, late, frac = [], [], []
for j in range(1, len(E)):
    p, e = E[j - 1], E[j]
    if not lo <= e['cut'] < hi or not e['inflight']:
        continue
    r = p['ack'] - p['q']
    if p['q'] <= e['cut'] <= p['ack'] and r > 0:
        frac.append((e['cut'] - p['q']) / r)
        early.append(e['cut'] - p['q'])
        late.append(p['ack'] - e['cut'])
print(f'early cuts inside the round in flight n {len(frac)}: cut at p50 {pc(early, .5)} ms after its append; batch then '
      f'sits frozen p50 {pc(late, .5)} p90 {pc(late, .9)} p99 {pc(late, .99)} ms until the round ends; share of the '
      f'in-flight round that ran with the next batch already frozen: {100 * sum(late) / sum(x + y for x, y in zip(early, late)):.0f}%')
# share of wall time during which a newly admitted Human must wait for the batch after the frozen one
frozen_ms = sum(max(0, E[j]['q'] - E[j]['cut']) for j in range(1, len(E)) if lo <= E[j]['cut'] < hi)
print(f'wall time with a frozen batch waiting (a Human admitted then pays one more whole round): '
      f'{100 * frozen_ms / (hi - lo):.0f}%')
# (2) late join
def humans(rule):
    out = []
    for j in range(2, len(E)):
        e, p = E[j], E[j - 1]
        h = e['taken'][0]
        span = e['cut'] - p['cut']
        for k in range(h):
            t = e['cut'] - span * (k + .5) / h
            out.append(rule(t, p, e))
    return out


now = humans(lambda t, p, e: e['ap'] - t)
print(f'humans now (estimate): n {len(now)} p50 {pc(now, .5)} p90 {pc(now, .9)} p99 {pc(now, .99)} <=1 s '
      f'{100 * sum(x <= 1000 for x in now) / len(now):.0f}%')
for d in (0, 75, 150):
    # joins the frozen batch p when it was admitted at least d ms (its own deps read) before p's append
    xs = humans(lambda t, p, e: (p['ap'] - t) if t + d <= p['q'] else (e['ap'] - t))
    moved = sum(1 for x, y in zip(xs, now) if x != y)
    print(f'  late join into the frozen batch, joiner deps {d} ms: p50 {pc(xs, .5)} p90 {pc(xs, .9)} p99 {pc(xs, .99)} '
          f'<=1 s {100 * sum(x <= 1000 for x in xs) / len(xs):.0f}% (moved {100 * moved / len(xs):.0f}% of Humans)')
for f in (.75, .5, .33):
    xs = [x * f for x in humans(lambda t, p, e: (p['ap'] - t) if t + 75 <= p['q'] else (e['ap'] - t))]
    print(f'  late join (75 ms) and every round x{f}: p50 {pc(xs, .5)} p90 {pc(xs, .9)} p99 {pc(xs, .99)}')
for f in (.75, .5, .33):
    xs = [x * f for x in now]
    print(f'  no late join, every round x{f}: p50 {pc(xs, .5)} p90 {pc(xs, .9)} p99 {pc(xs, .99)}')
# (1) legs two-way
ecmd = sorted((e['q'], e['ack'], sum(e['taken'])) for e in E)
eq = [x[0] for x in ecmd]
RL = rows[s]
sent = {(r[2], int(float(r[3]))): r[0] for r in RL if r[1] == 23 and r[2] < 192}
ans = {(r[2], int(float(r[3]))): r[0] for r in RL if r[1] == 21 and r[2] < 192}
lent = sorted((r[0], int(float(r[5]))) for r in RL if r[1] == 25)
lts = [x[0] for x in lent]
allv = []
for f in sorted({p for p, _ in sent}):
    left = {int(float(r[3])): (r[0], int(float(r[4]))) for r in rows[f] if r[1] == 24 and r[2] == s}
    ent = {int(float(r[3])): int(float(r[5])) for r in rows[f]
           if r[1] == 25 and int(float(r[4])) % 64 == 1 and int(float(r[3])) in left}
    for (p, c), t in sent.items():
        if p != f or c not in left or (p, c) not in ans or c not in ent or not lo <= t < hi:
            continue
        j = bisect.bisect_right(eq, t) - 1
        if j < 0 or not ecmd[j][0] <= t <= ecmd[j][1] or t - ecmd[j][0] > 5:
            continue  # only the call that carried the entry (sent in the append's own millisecond or just after)
        end = ans[(p, c)]
        lin = bisect.bisect_right(lts, end) - bisect.bisect_left(lts, t)
        allv.append({'f': f, 'outside': end - t - (left[c][0] - left[c][1]), 'cmds': ecmd[j][2], 'open': ent[c],
                     'lrate': 1000 * lin / max(1, end - t)})
print(f'entry-carrying voter calls n {len(allv)}: outside the handler p50 {pc([v["outside"] for v in allv], .5)} p90 '
      f'{pc([v["outside"] for v in allv], .9)} p99 {pc([v["outside"] for v in allv], .99)}')
cb = lambda v: '<=32' if v['cmds'] <= 32 else '33-63' if v['cmds'] < 64 else '>=64'
ob = lambda v: '<=5' if v['open'] <= 5 else '6-30' if v['open'] <= 30 else '31-60' if v['open'] <= 60 else '>60'
t2 = collections.defaultdict(list)
for v in allv:
    t2[(cb(v), ob(v))].append(v['outside'])
print('  outside-the-handler p50 / p90 (n), rows = commands in the entry, columns = follower open requests at entry')
for c in ('<=32', '33-63', '>=64'):
    print(f'   cmds {c:6}', ' | '.join(f'open {o}: {pc(t2[(c, o)], .5)} / {pc(t2[(c, o)], .9)} ({len(t2[(c, o)])})'
                                       for o in ('<=5', '6-30', '31-60', '>60') if t2[(c, o)]))
lb = collections.defaultdict(list)
for v in allv:
    lb['<150' if v['lrate'] < 150 else '150-250' if v['lrate'] < 250 else '250-350' if v['lrate'] < 350 else '>=350'].append(v['outside'])
print('  by leader inputs/s during the leg:', {k: (len(lb[k]), pc(lb[k], .5), pc(lb[k], .9)) for k in ('<150', '150-250', '250-350', '>=350') if lb[k]})
