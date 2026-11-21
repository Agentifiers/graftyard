"""Scratch: what the leader's silences are. A silence is a gap >= 20 ms between consecutive distinct stamps on the
leader's trace (its clock moves only on I/O; inputs arrive at 170-330/s). Each is tagged, in this order:
  sync     a leader sync() was pending across the whole gap and resolved at the gap's end stamp (+-2 ms)
  in-sync  a leader sync() was pending across the whole gap (it resolved later)
  apply    the stamp that starts the gap carries an Applied row (the turn applied an entry: resolutions, events)
  append   the stamp that starts the gap carries an AppendQueued row (cut, decide, encode, send)
  other
and the inputs that share the gap's end stamp are counted (a clump = inputs delivered together after the silence)."""
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
        if k not in seen and lo <= r[0] < hi: seen.add(k); rows.append(r)
by = collections.defaultdict(list)
for r in rows: by[r[0]].append(r)
stamps = sorted(by)
syncs = sorted((r[0] - int(float(r[5])), r[0]) for r in rows if r[1] == 31)
sb = [x[0] for x in syncs]
tot = collections.Counter(); cnt = collections.Counter(); clump = collections.defaultdict(list)
for i in range(len(stamps) - 1):
    g0, g1 = stamps[i], stamps[i + 1]
    if g1 - g0 < 20: continue
    kinds = {r[1] for r in by[g0]}
    j = bisect.bisect_right(sb, g0 + 2)
    pend = [s for s in syncs[max(0, j - 6):j] if s[1] >= g1 - 2]
    if any(abs(s[1] - g1) <= 2 for s in pend): tag = 'sync resolves at its end'
    elif pend: tag = 'inside a pending sync'
    elif 4 in kinds: tag = 'after an apply turn'
    elif 12 in kinds: tag = 'after an append turn'
    else: tag = 'other'
    tot[tag] += g1 - g0; cnt[tag] += 1
    clump[tag].append(sum(1 for r in by[g1] if r[1] == 25))
T = sum(tot.values())
print(f'leader silences >= 20 ms: {sum(cnt.values())}, {T / 1000:.0f} s of 240 s')
for tag in ('sync resolves at its end', 'inside a pending sync', 'after an apply turn', 'after an append turn', 'other'):
    if cnt[tag]:
        print(f'  {tag:26} n {cnt[tag]:5} {tot[tag] / 1000:6.1f} s ({100 * tot[tag] / T:2.0f}%) mean {tot[tag] / cnt[tag]:4.0f} ms | inputs at the stamp that ends it p50 {pc(clump[tag], .5)} p90 {pc(clump[tag], .9)}')
ent = collections.Counter(r[0] for r in rows if r[1] == 25)
big = sum(v for v in ent.values() if v >= 5)
print(f'inputs delivered in clumps of >= 5 at one stamp: {100 * big / max(1, sum(ent.values())):.0f}% of {sum(ent.values())}; >= 20: {100 * sum(v for v in ent.values() if v >= 20) / max(1, sum(ent.values())):.0f}%')
