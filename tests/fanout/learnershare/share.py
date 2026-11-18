#!/usr/bin/env python3
"""(b)'s first measurement (tests/am1/results/learner-share.md): of m51's learner index reads (ReadQuery 26 with ms 1),
the share a response to another call from the learner to the leader could have granted, so needed no read-index batch.
A read is grantable if a non-index call (CallSent 23 not at an IndexSent 1 instant, not a Hedge 11 resend) left the
learner after the read arrived and no later than the read's own batch left; held up to D ms, if one left within D.
A batch is avoidable only if every read in it is grantable. Windows on Cloudflare's clock: pre-change is the load
before the leader change (leader site 0), from the orb-stamped load start 11:16:37Z plus 7.4 s to site a's StepDown
alarm at 11:17:42Z; post-change is 3 minutes on site 1, the window the read gate's retries inflated.
  xz -dc tests/fanout/raw/tsave-m51b33.jsonl.xz > /tmp/m51/tsave.jsonl
  python3 tests/fanout/learnershare/share.py /tmp/m51/tsave.jsonl"""
import bisect, collections, datetime, json, sys
rows = collections.defaultdict(set)
for line in open(sys.argv[1]):
    r = json.loads(line)
    if "rows" in r and "learner" in r:
        rows[r["learner"]].update(tuple(x) for x in r["rows"])
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
W = {"pre-change": ("2026-11-18T11:16:44Z", "2026-11-18T11:17:42Z", 0),
     "post-change": ("2026-11-18T11:18:30Z", "2026-11-18T11:21:30Z", 1)}
HOLD = (0, 50, 100, 250, 500)
for w, (a, b, ldr) in W.items():
    T0, T1 = iso(a), iso(b)
    n = 0; g = collections.Counter(); batches = av = 0; calls = idxs = 0
    for s in sorted(rows):
        v = sorted(rows[s])
        idx = sorted(x[0] for x in v if x[1] == 1 and x[2] == ldr)
        ids, hedge = set(idx), {x[0] for x in v if x[1] == 11}
        other = [x[0] for x in v if x[1] == 23 and x[2] == ldr and x[0] not in ids and x[0] not in hedge]
        calls += sum(T0 <= t <= T1 for t in other); idxs += sum(T0 <= t <= T1 for t in idx)
        per = collections.defaultdict(lambda: [0, 0])
        for x in v:
            if x[1] != 26 or x[5] != 1 or not T0 <= x[0] <= T1:
                continue
            k = bisect.bisect_left(idx, x[0])
            if k == len(idx):
                continue
            n += 1
            j = bisect.bisect_left(other, x[0])
            nxt = other[j] if j < len(other) else 1 << 62
            own = per[idx[k]]; own[0] += 1
            if nxt <= idx[k]:
                g[0] += 1; own[1] += 1
            for D in HOLD[1:]:
                if nxt - x[0] <= D or nxt <= idx[k]:
                    g[D] += 1
        batches += len(per); av += sum(1 for t, c in per.values() if t == c)
    secs = (T1 - T0) / 1000
    print(f"{w} {a}..{b}: index reads {n} ({n / secs:.0f}/s over 4 learners), read-index batches {idxs / secs:.1f}/s, "
          f"other calls to the leader {calls / secs:.1f}/s")
    print(f"  grantable by a call that left anyway: {100 * g[0] / n:.1f}% of reads; batches avoidable {av} of {batches} "
          f"({100 * av / max(batches, 1):.1f}%)")
    print("  held for the next call: " + ", ".join(f"{D} ms {100 * g[D] / n:.1f}%" for D in HOLD[1:]))

# The hold policy's call count: a read waits up to D ms for the next other call; reads none reaches go out in one
# read-index batch at the first such read's arrival + D, which also carries every read queued by then. Batches per
# second against today's, with the other calls' times as they were (they are held-append acks and would not move).
print("hold policy, read-index batches/s over 4 learners (today's in the header lines):")
for w, (a, b, ldr) in W.items():
    T0, T1 = iso(a), iso(b); out = []
    for D in HOLD[1:]:
        nb = 0
        for s in sorted(rows):
            v = sorted(rows[s])
            ids, hedge = {x[0] for x in v if x[1] == 1 and x[2] == ldr}, {x[0] for x in v if x[1] == 11}
            other = [x[0] for x in v if x[1] == 23 and x[2] == ldr and x[0] not in ids and x[0] not in hedge]
            due = None
            for t in (x[0] for x in v if x[1] == 26 and x[5] == 1 and T0 <= x[0] <= T1):
                if due is not None and t > due:
                    nb += 1; due = None
                j = bisect.bisect_left(other, t)
                if j < len(other) and other[j] - t <= D and (due is None or other[j] <= due):
                    continue
                if due is None:
                    due = t + D
            nb += due is not None
        out.append(f"{D} ms {nb / ((T1 - T0) / 1000):.1f}/s")
    print(f"  {w}: " + ", ".join(out))

# Control: the same D-ms window with no riding (LEARNER_READ_EVERY raised to D), so the gain from riding is the gap.
print("control, the same window with no riding, read-index batches/s:")
for w, (a, b, ldr) in W.items():
    T0, T1 = iso(a), iso(b); out = []
    for D in HOLD[1:]:
        nb = 0
        for s in sorted(rows):
            due = None
            for t in sorted(x[0] for x in rows[s] if x[1] == 26 and x[5] == 1 and T0 <= x[0] <= T1):
                if due is not None and t > due:
                    nb += 1; due = None
                if due is None:
                    due = t + D
            nb += due is not None
        out.append(f"{D} ms {nb / ((T1 - T0) / 1000):.1f}/s")
    print(f"  {w}: " + ", ".join(out))
