#!/usr/bin/env python3
"""mergefront.py TAG... -> `merge-front`'s pricing (`decisions/DECISIONS.md`, "`merge-front`"): each B0-29 run's
merge:only p99 now, with a merge's identity reads alone, with a guess on every merge, and with both. A model over each
merge's measured `server-timing` stages (`tests/fanout/raw/a1-TAG.jsonl`, `mergeSteps`), not a measurement:
- front is `total` less `merge_plan` and `commit`; with a guess (`authz_ahead`) its reads are max(auth, resolve, authz,
  tier), else max(auth, resolve) then max(authz, tier), and the front's untimed rest is kept as measured;
- alone: each identity stage loses its `rcwait` less one READ_MS read per answer it shared, but no less than one read;
- a guess on a merge with none: `resolve` at half its serial time (its three reads at once) plus LINK_MS for the
  data center's link match, its reads ahead beside it.
At n under 100 a run's p99 is its slowest merge. With three tags it prints the median of their p99s and the pooled p99.
Run from the repository root.
"""
import json
import re
import sys

LINK_MS, READ_MS = 50, 70
ARMS = ('now', 'alone', 'guess', 'both')


def pc(x, q):
    x = sorted(x)
    return x[min(len(x) - 1, int(q * len(x)))]


def merges(tag):
    d = json.loads(open(f'tests/fanout/raw/a1-{tag}.jsonl').read().strip().split('\n')[-1])
    for s in d['mergeSteps']:
        t = {k: int(v) for k, v in re.findall(r'(\w+);dur=(\d+)', s.get('timing') or '')}
        if 'total' in t:
            yield s['ms'], t


def priced(ms, t):
    g = 'authz_ahead' in t
    front = t['total'] - t.get('merge_plan', 0) - t.get('commit', 0)
    az, tr = (t['authz_ahead'], t['tier_ahead']) if g else (t.get('authz', 0), t.get('tier', 0))
    auth, rv = t.get('auth', 0), t.get('resolve', 0)

    def reads(a, r, z, tt, gg):
        return max(a, r, z, tt) if gg else max(a, r) + max(z, tt)

    rest = front - reads(auth, rv, az, tr, g)
    save = max(t.get('rcwait', 0) - READ_MS * t.get('rcshare', 0), 0)

    def cut(x):
        return max(x - save, min(x, READ_MS))

    def at(a, r, z, tt, gg):
        return ms - front + reads(a, r, z, tt, gg) + rest

    rv2 = rv // 2 + LINK_MS
    return {
        'now': ms,
        'alone': at(cut(auth), cut(rv), cut(az), tr, g),
        'guess': ms if g else at(auth, rv2, az, tr, True),
        'both': at(cut(auth), cut(rv if g else rv2), cut(az), tr, True),
    }


def main():
    per, pool = {}, {k: [] for k in ARMS}
    for tag in sys.argv[1:]:
        rows = [priced(ms, t) for ms, t in merges(tag)]
        per[tag] = {k: max(r[k] for r in rows) for k in ARMS}
        for k in ARMS:
            pool[k] += [r[k] for r in rows]
        print(tag, 'n', len(rows), 'p99', per[tag])
    if len(per) == 3:
        print('median of run p99s', {k: sorted(p[k] for p in per.values())[1] for k in ARMS})
        print('pooled p99', {k: pc(v, 0.99) for k, v in pool.items()})


if __name__ == '__main__':
    main()
