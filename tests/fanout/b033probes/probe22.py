"""Scratch: leader inputs by route per 10 s, beside the agent lane's backlog (agent commands left queued after the cuts
of that 10 s, Trace::Cut) . Do outcome asks per /commit rise with the backlog?"""
import collections, json, lzma, sys
from datetime import datetime
def ms(s): return int(datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp() * 1000)
def pc(xs, q):
    xs = sorted(xs); return round(xs[min(len(xs) - 1, int(q * len(xs)))]) if xs else 0
path, a = sys.argv[1], ms(sys.argv[2])
names = {1: 'raft', 2: 'read', 3: 'commit', 5: 'outcome', 6: 'obr'}
rows, seen = [], set()
for line in lzma.open(path, 'rt'):
    d = json.loads(line)
    if 'rows' not in d or 'learner' in d or d.get('site') != 0: continue
    for r in d['rows']:
        k = tuple(r)
        if k not in seen and a <= r[0] < a + 300000: seen.add(k); rows.append(r)
per = collections.defaultdict(collections.Counter); left = collections.defaultdict(list); codes = collections.Counter()
for r in rows:
    b = (r[0] - a) // 10000
    if r[1] == 25:
        c = int(float(r[4])) % 64; codes[c] += 1; per[b][names.get(c, 'other')] += 1
    elif r[1] == 29:
        left[b].append((int(float(r[4])) >> 39) & 0x1fff)
print('route codes seen:', dict(sorted(codes.items())))
pts = []
for b in range(3, 27):
    c = per[b]; asks = c['outcome'] + c['obr']
    pts.append((pc(left[b], .5), asks / max(1, c['commit']), sum(c.values()) / 10, c['obr'] / 10))
pts.sort()
n = len(pts)
for name, sl in (('lowest third by backlog', pts[:n // 3]), ('middle', pts[n // 3:2 * n // 3]), ('highest third', pts[2 * n // 3:])):
    print(f'  10 s windows, {name:24}: agent commands left queued p50 {pc([x[0] for x in sl], .5):4} | outcome asks per /commit {sum(x[1] for x in sl) / len(sl):.2f} '
          f'| /outcome_by_request per s {sum(x[3] for x in sl) / len(sl):.1f} | leader inputs/s {sum(x[2] for x in sl) / len(sl):.0f}')
