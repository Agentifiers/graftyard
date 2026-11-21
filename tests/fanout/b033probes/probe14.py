"""Scratch: was the leader running during a round? The isolate clock moves only across I/O, so a turn's rows share one
stamp and a long turn (or a held input gate) shows as a gap between consecutive distinct stamps on the leader's trace.
With 170-330 traced inputs/s the gaps bound turn length to a few ms. Per entry round: the gap right after the append's
stamp (the appending turn: cut, decide, encode), all gaps >= 20 ms inside the round, and what is left."""
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
R = sorted(rows, key=lambda r: r[0])
stamps = sorted({r[0] for r in R})
gaps_all = [stamps[i + 1] - stamps[i] for i in range(len(stamps) - 1) if lo <= stamps[i] < hi]
print(f'leader distinct stamps {len(gaps_all)} in 240 s; gap p50 {pc(gaps_all, .5)} p99 {pc(gaps_all, .99)} max {max(gaps_all)}; time in gaps >= 20 ms: '
      f'{100 * sum(g for g in gaps_all if g >= 20) / 240000:.0f}% of the window, >= 50 ms: {100 * sum(g for g in gaps_all if g >= 50) / 240000:.0f}%, >= 100 ms: {100 * sum(g for g in gaps_all if g >= 100) / 240000:.0f}%')
first, acks = {}, collections.defaultdict(list)
for pos, r in enumerate(R):
    if r[1] == 12 and r[2] < 192: first.setdefault(int(float(r[4])), (r[0], int(float(r[3])), pos))
    if r[1] == 14 and r[2] < 192 and int(float(r[4])) == 0: acks[int(float(r[3]))].append(r[0])
idxs = sorted(first, key=lambda i: first[i][2]); apos = [first[i][2] for i in idxs]
cmds = {}
for pos, r in enumerate(R):
    if r[1] == 29:
        k = bisect.bisect_left(apos, pos) - 1
        if k >= 0:
            bt = int(float(r[3])); cmds.setdefault(idxs[k] + (r[2] // 16) % 4 + 1, sum((bt >> (12 * i)) & 0xfff for i in range(4)))
E = []
for i in idxs:
    t, rnd, _ = first[i]
    ack = min((x for x in acks.get(rnd, []) if x >= t), default=None)
    if ack is None or i not in cmds or not lo <= t < hi: continue
    j0, j1 = bisect.bisect_left(stamps, t), bisect.bisect_right(stamps, ack)
    g = [stamps[k + 1] - stamps[k] for k in range(j0, min(j1, len(stamps)) - 1)]
    after = stamps[j0 + 1] - stamps[j0] if j0 + 1 < len(stamps) else 0
    last = g[-1] if g else 0   # the gap that ends at the ack's stamp: how long the leader was away before it took the ack
    E.append({'round': ack - t, 'n': cmds[i], 'after': min(after, ack - t), 'last': last, 'silent': sum(x for x in g if x >= 20),
              'big': max(g, default=0)})
def show(name, v):
    g = lambda k, q=.5: pc([x[k] for x in v], q)
    print(f'  {name:14} n {len(v):3} round p50 {g("round")} p90 {g("round", .9)} | appending turn (gap after the append stamp) p50 {g("after")} p90 {g("after", .9)} | '
          f'gap ending at the ack p50 {g("last")} p90 {g("last", .9)} | all leader gaps >= 20 ms in the round p50 {g("silent")} p90 {g("silent", .9)} = '
          f'{100 * sum(x["silent"] for x in v) / max(1, sum(x["round"] for x in v)):.0f}% of round time | round minus them p50 {pc([x["round"] - x["silent"] for x in v], .5)} p90 {pc([x["round"] - x["silent"] for x in v], .9)}')
show('all', E)
for lo_, hi_ in ((0, 16), (17, 32), (33, 63), (64, 999)):
    w = [x for x in E if lo_ <= x['n'] <= hi_]
    if w: show(f'cmds {lo_}-{hi_}', w)
E.sort(key=lambda x: x['round']); n = len(E)
show('slowest 10%', E[int(.9 * n):])
