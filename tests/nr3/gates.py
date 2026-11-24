#!/usr/bin/env python3
# narrow-reads-3 (operator after the gate 16 A/B): is the slow part of a learner's read-index round trip, the part
# outside the leader's handler, the Durable Object output gate or the leader's input queue? From a run's traces
# alone: tests/nr3/raw/trace-TAG.json (split.mjs) and tsave-TAG.jsonl (the leader's rows saved each minute).
#   python3 tests/nr3/gates.py abwith
# Per learner call: S send (learner clock, x-graft-sent), A handler start on the leader (CallArrived 20), L handler
# return (x-graft-left), R answer in hand (AnswerArrived 21, learner clock).
#   rest  = (R - S) - (L - A): everything outside the leader's handler; skew-free, as each clock is differenced alone.
#   gateL = the leader's output gate on this answer: from L to the end of the leader confirm (Confirm 10, the
#           interval [at - ms, at]) that was running at L; 0 when none was. Confirms end in order, so that one covers
#           every write made before L. Leader clock only.
#   gateS = the learner's output gate on its own call: from S to the end of the learner confirm running at S.
#           Learner clock only.
#   resid = rest - gateL - gateS: both wires, the leader's input queue, and the learner taking its answer.
# The leader's input queue, on its clock: idle = A minus the last leader row before A (when the isolate last showed
# activity), and atGate = A minus the last leader confirm end at or before A. An input held behind the leader shows
# as arrivals bunched just after the leader's own activity.
import bisect, collections, json, os, sys

sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'fanout')))
from tracerows import merge

tag = sys.argv[1]
D = json.load(open(f"tests/nr3/raw/trace-{tag}.json"))
lead = next(k for k in D if k.startswith("f"))
f = [D[lead]]
try:
    for l in open(f"tests/nr3/raw/tsave-{tag}.jsonl"):
        r = json.loads(l)
        if f"f{r.get('site')}" == lead and "rows" in r:
            f.append(r["rows"])
except FileNotFoundError:
    pass
f = sorted(merge((0, s) for s in f)[0], key=lambda r: r[0])
L0 = int(lead[1:])


def pct(xs, q):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(q * len(xs)))] if xs else None


def confirms(tr):
    c = sorted((at - ms, at) for at, kind, peer, batch, index, ms in tr if kind == 10)
    return c, [x[0] for x in c]


def gate(c, starts, t):
    # The confirm running at t (began <= t < end): the latest-begun one before t whose end is past t.
    j = bisect.bisect_right(starts, t)
    best = 0
    for k in range(max(0, j - 8), j):
        if c[k][0] <= t < c[k][1]:
            best = max(best, c[k][1] - t)
    return best


fc, fs = confirms(f)
arr = {}
for at, kind, peer, batch, index, ms in f:
    if kind == 20:
        arr[(peer, index)] = at
ts = sorted({r[0] for r in f if r[1] != 20})
ends = sorted(c[1] for c in fc)
rows_all = []
print(f"{tag}: leader {lead}, {len(f)} rows, {len(fc)} confirms (p50/p99 {pct([b - a for a, b in fc], .5)}/{pct([b - a for a, b in fc], .99)} ms)")
print("learner  n     | rest p50/p90/p99 | gateL p50/p90/p99 (share of rest>=p90) | gateS p50/p90/p99 | resid p50/p90/p99")
for s in sorted(k for k in D if k.startswith("l")):
    a = 192 + int(s[1:])
    tr = D[s]
    sc, ss = confirms(tr)
    ended = collections.defaultdict(list)
    for at, kind, peer, batch, index, ms in tr:
        if kind == 2:
            ended[at].append(ms)
    rows = []
    for at, kind, peer, batch, index, ms in tr:
        if kind == 21 and index and peer == L0:
            for c in ended.get(at, []):
                S = at - c
                A = arr.get((a, S))
                if A is not None:
                    rest = (at - S) - (index - A)
                    gl, gs = gate(fc, fs, index), gate(sc, ss, S)
                    rows.append((rest, gl, gs, rest - gl - gs, A))
                    break
    rows_all += [(s,) + r for r in rows]
    r90 = pct([r[0] for r in rows], .9)
    tail = [r for r in rows if r[0] >= r90]
    share = 100 * sum(min(r[1], r[0]) for r in tail) / max(1, sum(r[0] for r in tail))
    col = lambda i: "%s/%s/%s" % (pct([r[i] for r in rows], .5), pct([r[i] for r in rows], .9), pct([r[i] for r in rows], .99))
    print(f"{s:7} {len(rows):5} | {col(0)} | {col(1)} ({share:.0f}%) | {col(2)} | {col(3)}")
# The input queue on the leader's clock: arrivals right after the leader's own last activity, or right after a confirm.
idle, atg = [], []
for s, rest, gl, gs, resid, A in rows_all:
    j = bisect.bisect_left(ts, A) - 1
    idle.append(A - ts[j] if j >= 0 else None)
    k = bisect.bisect_right(ends, A) - 1
    atg.append(A - ends[k] if k >= 0 else None)
print("leader input side, all learners: idle before arrival (A - last leader row) p10/p50/p90 %s/%s/%s; A - last confirm end p10/p50/p90 %s/%s/%s" % (
    pct([x for x in idle if x is not None], .1), pct([x for x in idle if x is not None], .5), pct([x for x in idle if x is not None], .9),
    pct([x for x in atg if x is not None], .1), pct([x for x in atg if x is not None], .5), pct([x for x in atg if x is not None], .9)))
# Does the residual's tail come with arrivals bunched on the leader's activity (a held input) or not?
r90, r50 = pct([r[4] for r in rows_all], .9), pct([r[4] for r in rows_all], .5)
hi = [i for i, r in enumerate(rows_all) if r[4] >= r90]
lo = [i for i, r in enumerate(rows_all) if r[4] < r50]
for name, ix in (("resid >= p90", hi), ("resid < p50", lo)):
    print(f"{name}: n {len(ix)}, arrivals within 2 ms of the last leader row {100 * sum(1 for i in ix if idle[i] is not None and idle[i] <= 2) / max(1, len(ix)):.0f}%, "
          f"within 2 ms of a confirm end {100 * sum(1 for i in ix if atg[i] is not None and atg[i] <= 2) / max(1, len(ix)):.0f}%")
