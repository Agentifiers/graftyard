"""Scratch: does a round's end line up with the leader's own sync() resolving? Steady window.
    python3 probe5.py TRACE.jsonl.xz SITE FROM_ISO SECONDS
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
first, acks = {}, collections.defaultdict(list)
for pos, r in enumerate(R):
    if r[1] == 12 and r[2] < 192:
        first.setdefault(int(float(r[4])), (r[0], int(float(r[3])), pos))
    if r[1] == 14 and r[2] < 192 and int(float(r[4])) == 0:
        acks[int(float(r[3]))].append((r[0], r[2], int(float(r[5]))))
idxs = sorted(first, key=lambda i: first[i][2])
apos = [first[i][2] for i in idxs]
cut = {}
for pos, r in enumerate(R):
    if r[1] != 29:
        continue
    k = bisect.bisect_left(apos, pos) - 1
    if k >= 0:
        batch = int(float(r[3]))
        cut.setdefault(idxs[k] + (r[2] // 16) % 4 + 1, sum((batch >> (12 * i)) & 0xfff for i in range(4)))
# sync rows: at = resolve time, ms = wall; batch = inputs | lag_begin<<20 | lag_end<<36; peer = ticks inside
syncs = []
for r in R:
    if r[1] == 31:
        bt = int(float(r[3]))
        syncs.append({'end': r[0], 'wall': int(float(r[5])), 'ticks': r[2], 'inputs': bt & 0xfffff,
                      'lag0': (bt >> 20) & 0xffff, 'lag1': (bt >> 36) & 0xffff, 'stmts': int(float(r[4])) & 0xfffff})
syncs.sort(key=lambda x: x['end'])
se = [x['end'] for x in syncs]
ans = sorted((r[0], r[2]) for r in R if r[1] == 21 and r[2] < 192)
at = [x[0] for x in ans]
lo, hi = a + 30000, a + 270000
Rd = []
for i in idxs:
    t, rnd, _ = first[i]
    ack = min((x for x in acks.get(rnd, []) if x[0] >= t), default=None)
    if ack is None or not lo <= t < hi:
        continue
    ack_t = ack[0]
    # the slowest-resolving sync that was pending at some point in the round
    j0 = bisect.bisect_left(se, t)
    inside = [x for x in syncs[j0:j0 + 12] if x['end'] - x['wall'] < ack_t and x['end'] > t]
    big = max(inside, key=lambda x: x['wall'], default=None)
    # first voter answer row in the round (leader clock)
    k = bisect.bisect_right(at, t)
    fa = at[k] if k < len(at) and at[k] <= ack_t else None
    Rd.append({'round': ack_t - t, 'cmds': cut.get(i), 'big': big, 'ack': ack_t, 't': t,
               'sync_to_ack': min((ack_t - x['end'] for x in inside if x['end'] <= ack_t), default=None),
               'covered': sum(min(x['end'], ack_t) - max(x['end'] - x['wall'], t) for x in inside), 'fa': fa})
Rd.sort(key=lambda x: x['round'])
n = len(Rd)
sy = [x['wall'] for x in syncs if lo <= x['end'] < hi]
print(f'leader sync() wall p50 {pc(sy, .5)} p90 {pc(sy, .9)} p99 {pc(sy, .99)} max {max(sy)} (n {len(sy)}); of syncs >= 200 '
      f'ms: {sum(1 for x in syncs if x["wall"] >= 200 and lo <= x["end"] < hi)}, with 0 ticker wakes inside '
      f'{sum(1 for x in syncs if x["wall"] >= 200 and lo <= x["end"] < hi and x["ticks"] == 0)}, inputs entered inside p50 '
      f'{pc([x["inputs"] for x in syncs if x["wall"] >= 200 and lo <= x["end"] < hi], .5)}, lag at end p50 '
      f'{pc([x["lag1"] for x in syncs if x["wall"] >= 200 and lo <= x["end"] < hi], .5)}')
for name, sl in (('all rounds', Rd), ('fastest half', Rd[:n // 2]), ('p50-p90', Rd[n // 2:int(.9 * n)]),
                 ('slowest 10%', Rd[int(.9 * n):]), ('slowest 3%', Rd[int(.97 * n):])):
    w = [x['big']['wall'] if x['big'] else 0 for x in sl]
    near = [x for x in sl if x['sync_to_ack'] is not None and x['sync_to_ack'] <= 5]
    cov = sum(x['covered'] for x in sl) / max(1, sum(x['round'] for x in sl))
    print(f'  {name:13} n {len(sl):3} round p50 {pc([x["round"] for x in sl], .5):5} | longest leader sync pending in it '
          f'p50 {pc(w, .5):4} p90 {pc(w, .9):4} | round ended within 5 ms of a sync resolving: {100 * len(near) / len(sl):3.0f}% | '
          f'share of round time with a leader sync pending {100 * cov:3.0f}% | round minus that sync p50 '
          f'{pc([x["round"] - (x["big"]["wall"] if x["big"] else 0) for x in sl], .5)} | cmds p50 '
          f'{pc([x["cmds"] for x in sl if x["cmds"] is not None], .5)}')
# counterfactual: every sync capped at 100 ms; round shortened by the excess of its longest pending sync when the round
# ended within 5 ms of a sync resolving
cf = sorted(x['round'] - (max(0, x['big']['wall'] - 100) if x['big'] and x['sync_to_ack'] is not None and
                         x['sync_to_ack'] <= 5 else 0) for x in Rd)
print(f'  rounds now p50 {pc([x["round"] for x in Rd], .5)} p90 {pc([x["round"] for x in Rd], .9)} p99 '
      f'{pc([x["round"] for x in Rd], .99)}; if a sync that a round ended on were capped at 100 ms: p50 {pc(cf, .5)} p90 '
      f'{pc(cf, .9)} p99 {pc(cf, .99)}')
