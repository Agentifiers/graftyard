import bisect, collections, json, lzma, sys
from datetime import datetime
def ms(s): return int(datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp() * 1000)
path, a = sys.argv[1], ms(sys.argv[2])
rows, seen = [], set()
for line in lzma.open(path, 'rt'):
    d = json.loads(line)
    if 'rows' not in d or 'learner' in d or d.get('site') != 0: continue
    for r in d['rows']:
        k = tuple(r)
        if k not in seen and a + 30000 <= r[0] < a + 270000 and r[1] == 29: seen.add(k); rows.append(r)
h = [int(float(r[3])) & 0xfff for r in rows]
ag = [(int(float(r[3])) >> 36) & 0xfff for r in rows]
n = len(h)
print(f'cuts {n}: with >=1 Human {100 * sum(x > 0 for x in h) / n:.0f}%; Humans per cut dist {dict(sorted(collections.Counter(min(x, 5) for x in h).items()))}; '
      f'Human-carrying cuts that also hit the 64-agent cap: {100 * sum(1 for x, y in zip(h, ag) if x > 0 and y >= 64) / max(1, sum(x > 0 for x in h)):.0f}%; '
      f'Humans riding a capped entry: {100 * sum(x for x, y in zip(h, ag) if y >= 64) / max(1, sum(h)):.0f}%')
