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
        if k not in seen and lo <= r[0] < hi: seen.add(k); rows[d.get('site')].append(r)
for s in sorted(rows):
    R = rows[s]
    lag = [int(float(r[5])) for r in R if r[1] == 8]
    ent = [r for r in R if r[1] == 25]
    routes = collections.Counter(int(float(r[4])) % 64 for r in ent)
    op = [int(float(r[5])) for r in ent]
    left = [(r[0] - int(float(r[4])), int(float(r[5]))) for r in R if r[1] == 24]
    sy = [int(float(r[5])) for r in R if r[1] == 31]
    cf = [int(float(r[5])) for r in R if r[1] == 10]
    print(f'site {s}: ticker Lag rows (>=20 ms) {len(lag)} sum {sum(lag)} ms max {max(lag, default=0)} | inputs/s {len(ent) / 240:.0f} '
          f'(raft {routes[1] / 240:.0f}, read {routes[2] / 240:.0f}, commit {routes[3] / 240:.0f}, outcome {routes[5] / 240:.0f}) | open at entry p50 '
          f'{pc(op, .5)} p90 {pc(op, .9)} | /raft handler ms p50 {pc([x[0] for x in left], .5)} p90 {pc([x[0] for x in left], .9)} p99 '
          f'{pc([x[0] for x in left], .99)}; loop lag at /raft entry p90 {pc([x[1] for x in left], .9)} p99 {pc([x[1] for x in left], .99)} max '
          f'{max((x[1] for x in left), default=0)} | sync wall p50 {pc(sy, .5)} p90 {pc(sy, .9)} p99 {pc(sy, .99)} (n {len(sy)}) confirm p90 {pc(cf, .9)}')
