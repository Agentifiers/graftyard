#!/usr/bin/env python3
"""readwait.py TSAVE FROM TO -> each learner's read-index wait for its index reads (`learner-cadence`'s cost to B0-34's
gate reads): from a read's arrival (`ReadQuery` 26 with `ms` 1, logged once its body is read) to the next batch the
learner sent (`IndexSent` 1), then that call's round trip (`IndexEnded` 2's `ms`). Batch numbers exceed 2^53 and lost
precision in the JSON trace, so sends are not told apart by number (a `CALL_DEADLINE` resend of the newest batch counts
as a send, which shortens a few reads' queue), and a call's end is matched to its send by time: `IndexEnded`'s `at`
less its `ms` within 3 ms of the send. The read's apply wait after the
answer is not counted. Times are each learner's own clock. FROM and TO are ISO instants."""
import bisect, collections, datetime, json, lzma, sys

P, F, T = sys.argv[1:4]
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
T0, T1 = iso(F), iso(T)
pct = lambda xs, q: (sorted(xs)[min(len(xs) - 1, int(q * len(xs)))] if xs else None)
rows = collections.defaultdict(set)
for line in (lzma.open if P.endswith(".xz") else open)(P, "rt"):
    r = json.loads(line)
    if "rows" in r and "learner" in r:
        rows[r["learner"]].update(tuple(x) for x in r["rows"] if x[1] in (1, 2, 26))
allq, allrt, allw = [], [], []
out = []
for l in sorted(rows):
    v = sorted(rows[l])
    sends = [x for x in v if x[1] == 1]
    ends = sorted((x[0] - x[5], x[5]) for x in v if x[1] == 2)
    starts = [e[0] for e in ends]
    rt = {}
    for s in sends:
        i = bisect.bisect_left(starts, s[0] - 3)
        if i < len(ends) and abs(ends[i][0] - s[0]) <= 3:
            rt[s[0]] = ends[i][1]
    st = [s[0] for s in sends]
    q, w, r_ = [], [], []
    for x in v:
        if x[1] != 26 or x[5] != 1 or not (T0 <= x[0] < T1):
            continue
        i = bisect.bisect_left(st, x[0])
        if i == len(st):
            continue
        wait = st[i] - x[0]
        q.append(wait)
        if st[i] in rt:
            r_.append(rt[st[i]])
            w.append(wait + rt[st[i]])
    allq += q; allrt += r_; allw += w
    out.append(f"l{l - 192} reads {len(q)}: queue p50 {pct(q, .5)} p99 {pct(q, .99)}, call p50 {pct(r_, .5)} p99 "
               f"{pct(r_, .99)}, queue+call p50 {pct(w, .5)} p90 {pct(w, .9)} p99 {pct(w, .99)} ms")
print(f"all learners, reads {len(allq)} ({len(allw)} matched to a call): queue p50 {pct(allq, .5)} p99 {pct(allq, .99)}, "
      f"call p50 {pct(allrt, .5)} p99 {pct(allrt, .99)}, queue+call p50 {pct(allw, .5)} p90 {pct(allw, .9)} p99 "
      f"{pct(allw, .99)} ms")
for o in out:
    print("  " + o)
