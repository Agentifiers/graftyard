# history-skew.py <key>.jsonl... : find reads in a pinned-client history that look stale on the recorded clocks, and test
# whether a clock offset between the client colos explains each one (B0-11, `results/m0-wp2.md`).
#
# A read is stale on the record when some write W2 was called after the write W it returned had completed, and W2
# completed before the read was called. Each colo stamps call and ret with its own clock, so across colos that order
# holds only up to the offset between them. For writer colo X and reader colo Y, a stale read with gap g (read.call minus
# W2.ret) needs off_Y - off_X > g. Every read in Y of a write from X bounds the same offset from above:
# off_Y - off_X < read.ret - write.call. Skew explains a stale read when one offset per colo meets every such bound and
# every stale read's gap within 10 minutes of it. Same-colo stale reads share one clock and cannot be skew.
import bisect, json, sys
from collections import Counter, defaultdict

stale, bound = [], defaultdict(list)
for path in sys.argv[1:]:
	rows = [json.loads(l) for l in open(path)]
	acked = [r for r in rows if r['op']['kind'] != 'get' and r['ok'] and (r['op']['kind'] == 'put' or (r.get('out') or {}).get('ok'))]
	by_value = {r['op']['value']: r for r in rows if r['op']['kind'] != 'get' and r['ok']}
	per_colo = defaultdict(list)
	for w in sorted(acked, key=lambda w: w['ret']): per_colo[w['colo']].append(w)
	rets = {colo: [x['ret'] for x in ws] for colo, ws in per_colo.items()}
	latest_call = {}  # per colo, the latest call among writes completed up to each index
	for colo, ws in per_colo.items():
		m, out = -1, []
		for x in ws: m = max(m, x['call']); out.append(m)
		latest_call[colo] = out
	for r in rows:
		if r['op']['kind'] != 'get' or not r['ok']: continue
		w = by_value.get((r.get('out') or {}).get('value'))
		if w is None: continue
		if w['colo'] != r['colo']: bound[(w['colo'], r['colo'])].append((r['call'], r['ret'] - w['call']))
		for colo, ws in per_colo.items():
			i = bisect.bisect_left(rets[colo], r['call']) - 1  # writes from colo that completed before the read
			if i >= 0 and latest_call[colo][i] > w['ret']:
				later = [x['ret'] for x in ws[max(0, i - 200):i + 1] if x['call'] > w['ret']]
				stale.append((colo, r['colo'], r['call'] - max(later), r['call']))
				break
for k in bound: bound[k].sort()
same = sum(1 for x, y, _, _ in stale if x == y)
def consistent(t):
	"""One offset per colo meeting every constraint within 10 minutes of t (difference constraints, Bellman-Ford)."""
	edges = []  # (u, v, c) means off_v - off_u <= c
	for (x, y), b in bound.items():
		near = b[bisect.bisect_left(b, (t - 600_000,)):bisect.bisect_left(b, (t + 600_000,))]
		if near: edges.append((x, y, min(v for _, v in near) - 1))
	edges += [(y, x, -(g + 1)) for x, y, g, t2 in stale if x != y and abs(t2 - t) <= 600_000]
	colos = {c for e in edges for c in e[:2]}
	d = dict.fromkeys(colos, 0)
	for _ in colos:
		for u, v, c in edges: d[v] = min(d[v], d[u] + c)
	return all(d[u] + c >= d[v] for u, v, c in edges)
unexplained = [s for s in stale if s[0] == s[1] or not consistent(s[3])]
gaps = sorted(g for _, _, g, _ in stale)
print(json.dumps({'staleReads': len(stale), 'sameColo': same, 'byPair': {f'{x}>{y}': n for (x, y), n in Counter((x, y) for x, y, _, _ in stale).items()},
	'gapMaxMs': gaps[-1] if gaps else None, 'gapP50Ms': gaps[len(gaps) // 2] if gaps else None,
	'unexplainedBySkew': len(unexplained), 'unexplained': unexplained[:10]}))
