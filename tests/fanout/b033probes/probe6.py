import bisect, collections, json, lzma, sys
from datetime import datetime
def ms(s): return int(datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp() * 1000)
path, s, a = sys.argv[1], int(sys.argv[2]), ms(sys.argv[3]); b = a + 300000
rows, seen = [], set()
for line in lzma.open(path, 'rt'):
    d = json.loads(line)
    if 'rows' not in d or 'learner' in d or d.get('site') != s: continue
    for r in d['rows']:
        k = tuple(r)
        if k not in seen and a <= r[0] < b: seen.add(k); rows.append(r)
R = sorted(rows, key=lambda r: r[0])
first, acks, fails = {}, collections.defaultdict(list), collections.defaultdict(list)
for pos, r in enumerate(R):
    if r[1] == 12 and r[2] < 192: first.setdefault(int(float(r[4])), (r[0], int(float(r[3])), pos))
    if r[1] == 14 and r[2] < 192:
        (acks if int(float(r[4])) == 0 else fails)[int(float(r[3]))].append((r[0], r[2], int(float(r[5]))))
rd = []
for i, (t, rnd, _) in first.items():
    ack = min((x for x in acks.get(rnd, []) if x[0] >= t), default=None)
    if ack and a + 30000 <= t < a + 270000: rd.append((ack[0] - t, i, rnd, ack[1], ack[2], len(acks[rnd]), len(fails.get(rnd, []))))
h = collections.Counter(min(x[0] // 100, 20) for x in rd)
print('round histogram, 100 ms bins:', [h[k] for k in range(21)])
ex = [x for x in rd if 990 <= x[0] <= 1010]
print('rounds within 990..1010 ms:', len(ex), 'of', len(rd), '; exactly 1000:', sum(1 for x in rd if x[0] == 1000))
print('  sample (round_ms, index, round, acking peer, AppendEnded ms since issued, oks, fails):', ex[:6])
print('  rounds whose first ok came from a call issued later than the first AppendQueued by >=900 ms:',
      sum(1 for x in rd if x[0] - x[4] >= 900))
gap = [x[0] - x[4] for x in rd]
print('  first-ok call was issued this long after the entry was first queued: p50', sorted(gap)[len(gap)//2], 'p90', sorted(gap)[int(.9*len(gap))], 'p99', sorted(gap)[int(.99*len(gap))])
print('AppendEnded failures in window:', sum(len(v) for v in fails.values()))
