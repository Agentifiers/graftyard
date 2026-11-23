#!/usr/bin/env python3
"""flushslice.py TAG [TAG ...]: what fills a repository leader's one-statement flush slices, from the row's leader trace
and Workers Logs extract, as `heavyturns.py` joins them (`tests/am1/results/seal-bytes-design.md`, section 8).

A one-statement flush slice is a leader `/raft` slice whose only write is one deferred transaction of at most two
statements (the ~128 KB `log` row) and that applies, queues, and sends nothing. Its CPU also pays for every learner
call it answers (`Trace::RaftCall`, kind 33: calls and answer bytes, most of them held AppendEntries). The script fits
CPU on calls and answer KB over the row's plain slices (no apply, flush, send or confirm), predicts each flush slice's
answers from that fit, and prints what is left: the flush itself and anything untraced. It also counts, per entry
replicated in the window, the AppendEntries copies the leader sent as calls (kind 12) and in learner answers (kind 19),
and the held bytes those answers carried. Run from the repository root.
"""
import collections, os, statistics as st, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
src = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'heavyturns.py')).read()
src = src.split('\nfor tag in sys.argv[1:]:')[0].replace(
    "        out.append(c)\n", "        c['rows'] = rows[i + 1:nxt[i + 1]]\n        out.append(c)\n")
tags, sys.argv = sys.argv[1:], ['heavyturns.py']
g = {'__file__': os.path.join(os.path.dirname(os.path.abspath(__file__)), 'heavyturns.py')}
exec(src, g)
M23 = (1 << 23) - 1


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
    one = [c for c in cs if c['flushed'] == 1 and c['flushStmts'] <= 2
           and not any(c[x] for x in ('applied', 'appends', 'stmts', 'sends'))]
    if not one:
        print(f'{tag}: no one-statement flush slices')
        continue
    pred = [a + b * c['calls'] + k * c['ans'] for c in one]
    left = [c['cpu'] - p for c, p in zip(one, pred)]
    print(f"{tag}: plain slices n {len(plain)}: CPU = {a:.2f} + {b:.3f} ms a call + {k:.4f} ms an answer KB")
    print(f"  one-statement flush slices n {len(one)}: CPU mean {st.mean(c['cpu'] for c in one):.1f} p50 "
          f"{st.median(c['cpu'] for c in one):.1f}; they answered {st.mean(c['calls'] for c in one):.1f} calls of "
          f"{st.mean(c['ans'] for c in one):.0f} KB; answers predict {st.mean(pred):.1f}; left mean {st.mean(left):.1f} "
          f"p50 {st.median(left):.1f}; with a storage confirm in the slice: {sum(1 for c in one if c['confirm'])}")
    rows = g['trace'](tag)
    t0 = g['ms'](g['RUNS'][tag][0])
    w = [r for r in rows if t0 + 90_000 <= r[0] < t0 + g['RUNS'][tag][1] * 1000]
    q, tk = [r for r in w if r[1] == 12], [r for r in w if r[1] == 19]
    idx = [r[4] for r in q + tk if r[4]]
    n = max(idx) - min(idx) + 1 if idx else 1
    held = sum((r[5] >> 23) & M23 for r in w if r[1] == 33) / 1024
    print(f"  per entry replicated (~{n}): {len(q) / n:.1f} call copies of {st.mean(r[5] for r in q) / 1024 if q else 0:.0f} KB, "
          f"{len(tk) / n:.1f} learner copies ({len(set(r[2] for r in tk))} learners), {held / n:.0f} KB held in answers")
