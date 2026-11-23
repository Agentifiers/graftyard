#!/usr/bin/env python3
"""learnercost.py TAG [TAG ...]: what a repository leader spends per entry on its learners, by learner region and by
kind of call, from the row's leader trace and Workers Logs extract as `heavyturns.py` joins them
(`tests/am1/results/learner-relay.md`).

A learner's `/raft` call to the leader (`Trace::RaftCall`, kind 33, caller 192 and up) either carries read-index asks
(`asks` > 0) or not (its own AppendEntries replies). Its answer carries held AppendEntries (`held_bytes`) and the rest
(`ReadIndexRes`, acks). CPU is priced by the row's plain-slice fit, as `flushslice.py` does: `a + b` a call and `k` an
answer KB. Learner regions follow `place_learners`' round-robin over LEARNERS (busiest first, ties by region order):
B0-33 guarded rows ran enam:2000,weur:1000,apac:1000 and B0-32 rows enam:4000,weur:2000,apac:2000 [inference from
`crates/types/src/placement.rs` and the drivers]; five are enam:1000,weur:2000,apac:2000
and three enam:1000,weur:1000,apac:1000. Run from the repository root.
"""
import collections, os, sys

here = os.path.dirname(os.path.abspath(__file__))
src = open(os.path.join(here, 'heavyturns.py')).read().split('\nfor tag in sys.argv[1:]:')[0].replace(
    "        out.append(c)\n", "        c['rows'] = rows[i + 1:nxt[i + 1]]\n        out.append(c)\n")
tags, sys.argv = sys.argv[1:], ['heavyturns.py']
g = {'__file__': os.path.join(here, 'heavyturns.py')}
exec(src, g)
M23 = (1 << 23) - 1
PLACE = {3: ['enam', 'weur', 'apac'], 4: ['enam', 'weur', 'apac', 'enam'], 5: ['weur', 'apac', 'enam', 'weur', 'apac'], 8: ['enam', 'weur', 'apac', 'enam', 'weur', 'apac', 'enam', 'enam']}


def lsq(x, y):
    k = len(x[0])
    m = [[sum(r[i] * r[j] for r in x) for j in range(k)] + [sum(r[i] * v for r, v in zip(x, y))] for i in range(k)]
    for c in range(k):
        p = max(range(c, k), key=lambda r: abs(m[r][c]))
        m[c], m[p] = m[p], m[c]
        for r in range(k):
            if r != c:
                f = m[r][c] / m[c][c]
                m[r] = [a - f * b for a, b in zip(m[r], m[c])]
    return [m[i][k] / m[i][i] for i in range(k)]


for tag in tags:
    cs = g['slices'](tag)
    for c in cs:
        rc = [r for r in c['rows'] if r[1] == 33]
        c['calls'], c['ans'] = len(rc), sum(r[5] & M23 for r in rc) / 1024
    plain = [c for c in cs if not any(c[k] for k in ('applied', 'flushed', 'stmts', 'sends', 'confirm')) and c['cpu'] < 100]
    a, b, k = lsq([[1, c['calls'], c['ans']] for c in plain], [c['cpu'] for c in plain])
    rows = g['trace'](tag)
    t0 = g['ms'](g['RUNS'][tag][0])
    lo, hi = t0 + 90_000, t0 + g['RUNS'][tag][1] * 1000
    w = [r for r in rows if lo <= r[0] < hi]
    idx = [r[4] for r in w if r[1] in (12, 19) and r[4]]
    n = max(idx) - min(idx) + 1
    slots = sorted({r[2] for r in w if r[1] == 33 and r[2] >= 192})
    region = dict(zip(slots, PLACE[len(slots)]))
    s = collections.defaultdict(collections.Counter)
    for r in w:
        if r[1] == 33 and r[2] >= 192:
            x = s[region[r[2]]]
            x['ask' if (r[4] >> 28) & 63 else 'reply'] += 1
            x['heldKB'] += ((r[5] >> 23) & M23) / 1024
            x['restKB'] += ((r[5] & M23) - ((r[5] >> 23) & M23)) / 1024
            x['learners'] = sum(1 for v in region.values() if v == region[r[2]])
    per = a + b
    print(f"{tag}: {n} entries in {(hi - lo) / 1000:.0f} s; fit {a:.2f} + {b:.3f} a call + {k:.4f} an answer KB "
          f"({per:.2f} ms a call); ms an entry:")
    tot = collections.Counter()
    for reg, x in sorted(s.items()):
        v = {'ask': per * x['ask'] / n, 'reply': per * x['reply'] / n, 'held': k * x['heldKB'] / n,
             'rest': k * x['restKB'] / n}
        tot.update(v)
        print(f"  {reg:4} ({x['learners']} learners): read-index calls {x['ask'] / n:4.1f} = {v['ask']:4.1f}, "
              f"append-reply calls {x['reply'] / n:4.1f} = {v['reply']:4.1f}, held entries {x['heldKB'] / n:4.0f} KB = "
              f"{v['held']:4.1f}, other answer bytes {v['rest']:4.1f}; all {sum(v.values()):4.1f}")
    print(f"  all learners: read-index {tot['ask']:.1f}, append replies {tot['reply']:.1f}, held entries "
          f"{tot['held']:.1f}, other {tot['rest']:.1f}; all {sum(tot.values()):.1f}")
