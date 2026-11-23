#!/usr/bin/env python3
"""readcalls.py TAG [TAG ...]: a repository leader's learner calls by kind, and what fewer of them would save per entry
(`tests/am1/results/readindex-batch.md`).

Reads the same leader trace and plain-slice fit as `learnercost.py`. Per learner: `/raft` calls a second that carry
read-index asks and calls that carry only append replies (`Trace::RaftCall`, kind 33). Then three ways to send fewer
calls, priced
at the fit's `a + b` a call (bytes unchanged, so the per-KB part is not counted):
- F: one ask per region per 50 ms, so at most 20 a second for all of a region's learners together;
- B: append replies ride the next ask (the reply bill, an upper bound);
- C: a 100 ms learner floor, at most 10 asks a second per learner.
Each 1 s bucket keeps at most the cap; the rest is saved. Run from the repository root.

`IndexTaken` (kind 5) is not used: its `batch` is above 2^53 and the trace's JSON rounds it, so `heavyturns.trace`'s
dedup merges different asks in the same millisecond. Within one snapshot the leader's taken rows equal RaftCall's asks.
"""
import collections, os, sys

here = os.path.dirname(os.path.abspath(__file__))
tags, sys.argv = sys.argv[1:], ['learnercost.py']
g = {'__file__': os.path.join(here, 'learnercost.py')}
exec(open(os.path.join(here, 'learnercost.py')).read(), g)
h, lsq, PLACE = g['g'], g['lsq'], g['PLACE']
M23 = (1 << 23) - 1
CPU = {'mrB': 136, 'm3fsf2-b0-32': 128, 'm3fg3-b0-32': 131}  # leader CPU per entry on all routes, learner-relay.md


def capped(counts, cap):
    return sum(max(0, v - cap) for v in counts.values())


for tag in tags:
    cs = h['slices'](tag)
    for c in cs:
        rc = [r for r in c['rows'] if r[1] == 33]
        c['calls'], c['ans'] = len(rc), sum(r[5] & M23 for r in rc) / 1024
    plain = [c for c in cs if not any(c[k] for k in ('applied', 'flushed', 'stmts', 'sends', 'confirm')) and c['cpu'] < 100]
    a, b, _ = lsq([[1, c['calls'], c['ans']] for c in plain], [c['cpu'] for c in plain])
    per = a + b
    rows = h['trace'](tag)
    t0 = h['ms'](h['RUNS'][tag][0])
    lo, hi = t0 + 90_000, t0 + h['RUNS'][tag][1] * 1000
    secs = (hi - lo) / 1000
    w = [r for r in rows if lo <= r[0] < hi]
    idx = [r[4] for r in w if r[1] in (12, 19) and r[4]]
    n = max(idx) - min(idx) + 1
    lr = [r for r in w if r[1] == 33 and r[2] >= 192]
    slots = sorted({r[2] for r in lr})
    region = dict(zip(slots, PLACE[len(slots)]))
    ask = [r for r in lr if (r[4] >> 28) & 63]
    reply = len(lr) - len(ask)
    voters = sum(1 for r in w if r[1] == 33 and r[2] < 192)
    per_l = collections.Counter((r[2], (r[0] - lo) // 1000) for r in ask)
    per_r = collections.Counter((region[r[2]], (r[0] - lo) // 1000) for r in ask)
    ms = lambda calls: per * calls / n  # noqa: E731
    f, c100 = capped(per_r, 20), capped(per_l, 10)
    cpu = CPU.get(tag)
    pct = lambda v: f' ({100 * v / cpu:.1f}% of {cpu} ms)' if cpu else ''  # noqa: E731
    print(f'{tag}: {n} entries in {secs:.0f} s, {n / secs:.1f} a second; {per:.2f} ms a call; {len(slots)} learners')
    print(f'  learner calls {len(lr) / secs:.1f}/s: asks {len(ask) / secs:.1f} ({len(ask) / secs / len(slots):.1f} a learner), '
          f'append replies {reply / secs:.1f}; voter calls {voters / secs:.1f}/s; asks a call max '
          f'{max((r[4] >> 28) & 63 for r in ask)}')
    print(f'  read-index calls {ms(len(ask)):.1f} ms an entry, append-reply calls {ms(reply):.1f}')
    print(f'  F region cap 20/s: saves {f / secs:.1f} calls/s = {ms(f):.1f} ms an entry{pct(ms(f))}')
    print(f'  B replies ride asks: saves up to {reply / secs:.1f} calls/s = {ms(reply):.1f}{pct(ms(reply))}')
    print(f'  F+B: {ms(f + reply):.1f}{pct(ms(f + reply))}')
    print(f'  C 100 ms floor: saves {c100 / secs:.1f} calls/s = {ms(c100):.1f}{pct(ms(c100))}')
    fa = [r for r in w if r[1] == 33 and r[2] < 192 and (r[4] >> 28) & 63]
    f50 = capped(collections.Counter((r[2], (r[0] - lo) // 1000) for r in fa), 20)
    print(f'  followers: read-index calls {len(fa) / secs:.1f}/s = {ms(len(fa)):.1f} ms an entry; a 50 ms follower floor '
          f'saves {f50 / secs:.1f} calls/s = {ms(f50):.1f}{pct(ms(f50))}')
