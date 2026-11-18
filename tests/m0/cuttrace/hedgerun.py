#!/usr/bin/env python3
"""dep-hedge-adaptive on a hosted run: replays each chain's hedge delay (`crates/lookup`, `dep_hedge_after`) over the
leader's `/deps_log` and reports, per chain, the reads, the share hedged, the extra reads a second, and the read p99.

`/deps_log` keeps every dependency read the leader used, in entry order, with the time the read took as the caller
saw it (`ms`, hedge included). The leader isolate did those reads itself, so its window is the same sequence: a read
was hedged when its time passed the delay its chain's window gave before it. The replay cannot see an isolate that
restarted (its window starts empty again) or reads dropped before `DepsUsed`; `/stats` `depHedges.asked` checks it.

    python3 tests/m0/cuttrace/hedgerun.py DEPS_LOG.json LOAD_START_MS [SECONDS]
"""
import collections
import json
import sys

FLOOR, CAP, WINDOW, MIN = 100, 1000, 64, 16


def pct(xs, q):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(len(xs) * q))] if xs else None


rows = json.load(open(sys.argv[1]))
t0 = int(sys.argv[2])
t1 = t0 + 1000 * int(sys.argv[3] if len(sys.argv) > 3 else 600)
win = collections.defaultdict(list)
per = collections.defaultdict(lambda: {'n': 0, 'hedged': 0, 'ms': [], 'delay': []})
for r in sorted(rows, key=lambda r: (r['at'], r['index'])):
    for x in r['reads']:
        if 'ms' not in x:
            continue
        chain, ms = str(x['chain']), int(x['ms'])
        w = win[chain]
        delay = CAP if len(w) < MIN else max(FLOOR, min(CAP, sorted(w)[len(w) * 9 // 10]))
        if t0 <= r['at'] < t1:
            p = per[chain]
            p['n'] += 1
            p['hedged'] += ms > delay
            p['ms'].append(ms)
            p['delay'].append(delay)
        w.append(ms)
        del w[:-WINDOW]

secs = (t1 - t0) / 1000
kinds = collections.defaultdict(lambda: {'n': 0, 'hedged': 0, 'ms': []})
out = {'chains': {}, 'kinds': {}}
for chain, p in sorted(per.items(), key=lambda kv: -kv[1]['n']):
    k = kinds[chain.split(':')[0]]
    k['n'] += p['n']
    k['hedged'] += p['hedged']
    k['ms'] += p['ms']
    out['chains'][chain] = {'reads': p['n'], 'hedged': p['hedged'], 'share': round(p['hedged'] / p['n'], 3),
                            'extraPerS': round(p['hedged'] / secs, 2), 'delayP50': pct(p['delay'], .5),
                            'readP50': pct(p['ms'], .5), 'readP90': pct(p['ms'], .9), 'readP99': pct(p['ms'], .99)}
for kind, k in kinds.items():
    out['kinds'][kind] = {'reads': k['n'], 'hedged': k['hedged'], 'share': round(k['hedged'] / k['n'], 3),
                          'extraPerS': round(k['hedged'] / secs, 2), 'readP50': pct(k['ms'], .5),
                          'readP99': pct(k['ms'], .99)}
n = sum(k['n'] for k in kinds.values())
h = sum(k['hedged'] for k in kinds.values())
out['all'] = {'reads': n, 'hedged': h, 'share': round(h / n, 3) if n else None, 'extraPerS': round(h / secs, 2)}
print(json.dumps(out, indent=1))
