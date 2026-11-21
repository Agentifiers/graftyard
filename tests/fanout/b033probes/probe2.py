"""Scratch: per-entry and per-leg split for the leader's round, steady-state window, plus human-weighted decomposition.
    python3 probe2.py TRACE.jsonl.xz SITE FROM_ISO SECONDS
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
app, first, acks, queued = {}, {}, collections.defaultdict(list), collections.defaultdict(list)
for pos, r in enumerate(R):
    if r[1] == 4:
        app.setdefault(int(float(r[4])), r[0])
    if r[1] == 12 and r[2] < 192:
        first.setdefault(int(float(r[4])), (r[0], int(float(r[3])), pos))
        queued[int(float(r[4]))].append((r[0], r[2], int(float(r[5]))))
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
    batch, left, m = int(float(r[3])), int(float(r[4])), int(float(r[5]))
    cut.setdefault(n, {'at': r[0], 'end': r[2] % 16, 'held': (r[2] // 16) % 4, 'inflight': r[2] >= 64,
                       'taken': [(batch >> (12 * i)) & 0xfff for i in range(4)],
                       'left': [(left >> (13 * i)) & 0x1fff for i in range(4)], 'bytes': (m >> 21) & 0x1fffff})
E = []
for i in idxs:
    if i not in cut:
        continue
    t, rnd, _ = first[i]
    ack = min((x for x in acks.get(rnd, []) if x[0] >= t), default=None)
    if ack is None:
        continue
    E.append({'i': i, 'cut': cut[i]['at'], 'q': t, 'ack': ack[0], 'who': ack[1], 'ap': max(app.get(i, ack[0]), ack[0]),
              'rnd': rnd, **cut[i]})
E.sort(key=lambda e: e['cut'])
# the big gaps
gaps = sorted(((E[j]['cut'] - E[j - 1]['cut'], (E[j - 1]['cut'] - a) / 1000) for j in range(1, len(E))), reverse=True)[:6]
print('largest cut-to-cut gaps (ms, at s into load):', [(g, round(t, 1)) for g, t in gaps])
print('first cut at s', (E[0]['cut'] - a) / 1000, 'last', (E[-1]['cut'] - a) / 1000)
# human arrivals per 30 s
hb = collections.Counter()
for e in E:
    hb[(e['cut'] - a) // 30000] += e['taken'][0]
print('humans taken per 30 s:', [hb[k] for k in range(10)])
ab = collections.Counter()
for e in E:
    ab[(e['cut'] - a) // 30000] += e['taken'][3]
print('agents taken per 30 s:', [ab[k] for k in range(10)])
# steady window
lo, hi = a + 30000, a + 270000
S = [e for e in E if lo <= e['cut'] < hi]
rounds = [e['ack'] - e['q'] for e in S]
print(f'steady 30-270 s: entries {len(S)} round p50 {pc(rounds, .5)} p90 {pc(rounds, .9)} p99 {pc(rounds, .99)} max '
      f'{max(rounds)}')
for name, key in (('cmds', lambda e: sum(e['taken'])), ('KB', lambda e: e['bytes'] // 1024)):
    edges = (16, 32, 63, 80, 10 ** 9) if name == 'cmds' else (16, 48, 96, 160, 10 ** 9)
    bins = collections.defaultdict(list)
    for e in S:
        v = key(e)
        bins[next(x for x in edges if v <= x)].append(e['ack'] - e['q'])
    print(f'  round by {name}:', {f'<={k}': (len(v), pc(v, .5), pc(v, .9)) for k, v in sorted(bins.items())})
print('  bytes per entry p50/p90/max KB', pc([e['bytes'] for e in S], .5) // 1024, pc([e['bytes'] for e in S], .9) // 1024,
      max(e['bytes'] for e in S) // 1024)
# legs
RL = rows[s]
sent = {(r[2], int(float(r[3]))): (r[0], int(float(r[4])), int(float(r[5]))) for r in RL if r[1] == 23 and r[2] < 192}
ans = {(r[2], int(float(r[3]))): r[0] for r in RL if r[1] == 21 and r[2] < 192}
legs = collections.defaultdict(list)
for f in sorted({p for p, _ in sent}):
    left = {int(float(r[3])): (r[0], int(float(r[4])), int(float(r[5]))) for r in rows[f] if r[1] == 24 and r[2] == s}
    ent = {}
    for r in rows[f]:
        if r[1] == 25 and int(float(r[4])) % 64 == 1 and int(float(r[3])) in left:
            ent[int(float(r[3]))] = int(float(r[5]))
    for (p, c), (t, waiting, lag) in sent.items():
        if p != f or c not in left or (p, c) not in ans or not lo <= t < hi:
            continue
        l_at, l_entry, l_lag = left[c]
        legs[f].append({'t': t, 'out': l_entry - t, 'h': l_at - l_entry, 'back': ans[(p, c)] - l_at,
                        'leg': ans[(p, c)] - t, 'lag': lag, 'flag': l_lag, 'open': ent.get(c), 'waiting': waiting})
for f, v in legs.items():
    g = lambda k, q: pc([x[k] for x in v], q)
    print(f'  leg {s}->{f} n {len(v)}: leg p50 {g("leg", .5)} p90 {g("leg", .9)} p99 {g("leg", .99)} | out (sent->follower '
          f'handler entry) min {min(x["out"] for x in v)} p50 {g("out", .5)} p90 {g("out", .9)} p99 {g("out", .99)} | '
          f'handler p50 {g("h", .5)} p90 {g("h", .9)} | back (handler return->leader AnswerArrived) min '
          f'{min(x["back"] for x in v)} p50 {g("back", .5)} p90 {g("back", .9)} p99 {g("back", .99)} | leader loop lag '
          f'at send p50 {g("lag", .5)} p90 {g("lag", .9)} | follower lag at entry p50 {g("flag", .5)} p90 '
          f'{g("flag", .9)} | follower open p50 {pc([x["open"] for x in v if x["open"] is not None], .5)}')
    # out and back by follower open requests
    for name in ('out', 'back'):
        bins = collections.defaultdict(list)
        for x in v:
            if x['open'] is None:
                continue
            bins[next(k for k in (5, 20, 40, 80, 10 ** 9) if x['open'] <= k)].append(x[name])
        print(f'    {name} by follower open requests:',
              {f'<={k}': (len(w), pc(w, .5), pc(w, .9)) for k, w in sorted(bins.items())})
# leader-side remainder of a round: round - fastest leg of its calls. Map calls to entries by time: calls sent in [q, ack].
allsent = sorted((t, p, c) for (p, c), (t, _, _) in sent.items())
ts = [x[0] for x in allsent]
rem, pre, post = [], [], []
for e in S:
    j0, j1 = bisect.bisect_left(ts, e['q']), bisect.bisect_right(ts, e['ack'])
    cs = [(t, p, c) for t, p, c in allsent[j0:j1] if (p, c) in ans and ans[(p, c)] <= e['ack'] + 1]
    if not cs:
        continue
    fa = min(ans[(p, c)] for _, p, c in cs)
    pre.append(min(t for t, _, _ in cs) - e['q'])
    post.append(e['ack'] - fa)
    rem.append((e['ack'] - e['q']) - (fa - min(t for t, _, _ in cs)))
print(f'  leader side of a round: AppendQueued->first CallSent p50 {pc(pre, .5)} p90 {pc(pre, .9)} p99 {pc(pre, .99)}; '
      f'first AnswerArrived->AppendEnded p50 {pc(post, .5)} p90 {pc(post, .9)} p99 {pc(post, .99)} (n {len(pre)})')
# human-weighted decomposition
W = []
for j in range(1, len(E)):
    e, h = E[j], E[j]['taken'][0]
    span = e['cut'] - E[j - 1]['cut']
    for k in range(h):
        t = e['cut'] - span * (k + .5) / h
        W.append((e['ap'] - t, e['cut'] - t, e['q'] - e['cut'], e['ack'] - e['q'], e['ap'] - e['ack'], sum(e['taken'])))
W.sort()
tot = [w[0] for w in W]
print(f'human-weighted n {len(W)}: p50 {pc(tot, .5):.0f} p90 {pc(tot, .9):.0f} p99 {pc(tot, .99):.0f} share<=1 s '
      f'{100 * sum(x <= 1000 for x in tot) / len(tot):.0f}%')
for l, h, name in ((.4, .6, 'p40-60'), (.85, .95, 'p85-95'), (.97, 1.0, 'top 3%')):
    xs = W[int(l * len(W)):int(h * len(W))]
    f = lambda k: round(sum(x[k] for x in xs) / len(xs))
    print(f'  {name} (n {len(xs)}): total {f(0)} = freeze wait {f(1)} + frozen, held behind entry in flight {f(2)} + own '
          f'round {f(3)} + apply {f(4)}; cmds in its entry {f(5)}')


def cf(name, fn):
    xs = [fn(w) for w in W]
    print(f'  what-if {name}: p50 {pc(xs, .5):.0f} p90 {pc(xs, .9):.0f} p99 {pc(xs, .99):.0f}')


cf('held-behind removed (late join / no early freeze), rounds unchanged', lambda w: w[0] - w[2])
cf('freeze wait + own round only, each round x0.5', lambda w: (w[1] + w[2] + w[3]) * .5 + w[4])
cf('each round x0.33', lambda w: (w[1] + w[2] + w[3]) / 3 + w[4])
