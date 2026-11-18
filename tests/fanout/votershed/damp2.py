#!/usr/bin/env python3
"""damp2.py ROWS.pkl FROM TO -> /suspect steps left with the proposed member-side damper: a report whose (colo, about,
term) got a stepped `promised` verdict less than GAP ms ago is answered from that verdict with no step; every other
verdict, and every first report of a key, steps. Replayed on `Reported` (28) rows, which hold only reports that
reached `verdict`. The rule's promise_until bound is not in the trace, so this is an upper bound on what it answers.
  python3 tests/fanout/votershed/damp2.py /tmp/shed/rows.pkl 2026-11-18T01:00:18Z 2026-11-18T01:05:18Z"""
import datetime, pickle, sys
rows = pickle.load(open(sys.argv[1], 'rb'))
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp() * 1000)
T0, T1 = iso(sys.argv[2]), iso(sys.argv[3])
secs = (T1 - T0) / 1000
PROMISED = 4
for s in sorted(rows):
    rep = sorted(x for x in rows[s] if x[1] == 28 and T0 <= x[0] <= T1)
    out = [f"member {s}: stepped {len(rep) / secs:.1f}/s"]
    for gap in (500, 1000, 2000):
        cached, n = {}, 0
        for x in rep:
            k = (x[4], x[2], x[3])
            if k in cached and x[0] < cached[k]:
                continue
            n += 1
            cached.pop(k, None)
            if x[5] == PROMISED:
                cached[k] = x[0] + gap
        out.append(f"gap {gap} -> {n / secs:.1f}/s ({1 - n / max(1, len(rep)):.0%} answered from the cache)")
    print("; ".join(out))
