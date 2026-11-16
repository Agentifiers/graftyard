#!/usr/bin/env python3
"""BF-54 job start, as run 2 measured it (tests/m4/results/m4-wp4.md, "BF-54 run 2"): each job's first step's
REST `started_at`, timed from its run's dispatch 201 event in the edge's Workers Logs (the end of the request, so
`RunCreated` had committed), and from the request's arrival (event time minus wall). Runs and dispatches pair in order.
    python3 tests/m4/raw/wp4/f77/start.py tests/m4/raw/wp4/bf54-run3
"""
import json, sys
from datetime import datetime
d = sys.argv[1]
ts = lambda s: datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp()
disp = [(ts(l.split()[0]), int(l.split()[1]) / 1000) for l in open(f'{d}/dispatch-times.txt') if l.strip()]
runs = [json.loads(l) for l in open(f'{d}/rest-jobs.jsonl') if l.strip()]
runs.sort(key=lambda r: r['run'])
assert len(runs) == len(disp), (len(runs), len(disp))
after, arrive, missing, concl = [], [], [], {}
for (t, wall), r in zip(disp, runs):
    for j in r['jobs']:
        concl[j['conclusion']] = concl.get(j['conclusion'], 0) + 1
        st = [s['started_at'] for s in j['steps'] if s.get('started_at')]
        if not st:
            missing.append(j['id']); continue
        s = ts(min(st))
        after.append((s - t, j['id'])); arrive.append((s - (t - wall), j['id']))
def pct(xs):
    v = sorted(x for x, _ in xs); n = len(v); at = lambda p: v[min(n - 1, int(p * n))]
    return f"n {n} p50 {at(.5):.1f} s, p95 {at(.95):.1f} s, p99 {at(.99):.1f} s, max {v[-1]:.1f} s (job {max(xs)[1]}); over 35 s {sum(x > 35 for x in v)}, over 60 s {sum(x > 60 for x in v)}"
print('conclusions', concl, 'jobs with no step', missing)
print('after the dispatch answered 201:', pct(after))
print('after the dispatch request arrived:', pct(arrive))
print('dispatch wall s: p50 %.1f, max %.1f' % (sorted(w for _, w in disp)[len(disp) // 2], max(w for _, w in disp)))
