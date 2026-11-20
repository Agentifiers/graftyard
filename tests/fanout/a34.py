#!/usr/bin/env python3
import json, lzma, sys, collections
# A34: did the leader's /commit and /outcome_by_request answers wait on a deferred flush?
# A `Flushed` row (kind 16) at F with ms m: the deferred transaction was deferred at F-m and ran at F, so `settled()`
# held every reply in [F-m, F] until F. A request `Entered` (kind 25) at A, route code index%64 (ROUTES: 2 /commit,
# 5 /outcome_by_request), is answered no earlier than max(A + hold, F) for the pending flush whose span holds A + hold
# (hold: 1000 ms for /commit's COMMIT_HOLD, about 0 for an ask). Ticker stalls (Lag, kind 8) are listed beside.
# Usage: a34.py TRACE.jsonl[.xz] T0_MS SECONDS
path, t0, secs = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
t1 = t0 + 1000 * secs
seen, ent, fl, lags = set(), [], [], []
for l in (lzma.open if path.endswith('.xz') else open)(path, 'rt'):
    d = json.loads(l)
    if 'rows' not in d or 'learner' in d or d.get('site') != 0:
        continue
    for r in d['rows']:
        t = tuple(r)
        if t in seen or not t0 - 5000 <= r[0] < t1 + 5000:
            continue
        seen.add(t)
        k = r[1]
        if k == 25:
            code = int(float(r[4])) % 64
            if code in (2, 5) and t0 <= r[0] < t1:
                ent.append((r[0], code))
        elif k == 16:
            fl.append((r[0] - int(float(r[5])), r[0]))
        elif k == 8 and int(float(r[5])) >= 200:
            lags.append((r[0], int(float(r[5]))))
fl.sort()
def flush_wait(a):
    w = 0
    for s, f in fl:
        if s <= a <= f:
            w = max(w, f - a)
    return w
out = collections.defaultdict(collections.Counter)
for at, code in ent:
    hold = 1000 if code == 2 else 0
    w = flush_wait(at + hold)
    b = 'none' if w == 0 else '<500' if w < 500 else '500-1000' if w < 1000 else '1000-2000' if w < 2000 else '>=2000'
    out['/commit' if code == 2 else '/outcome_by_request'][b] += 1
for r, c in out.items():
    print(r, sum(c.values()), dict(sorted(c.items())))
big = [(s - t0, f - s) for s, f in fl if t0 <= f < t1 and f - s >= 500]
print('flushes waiting >= 500 ms (start from T0 ms, wait ms):', big)
print('ticker stalls >= 200 ms (end from T0 ms, ms):', [(a - t0, m) for a, m in lags if t0 <= a < t1])
