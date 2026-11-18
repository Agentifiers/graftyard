#!/usr/bin/env python3
"""damp.py ROWS.pkl FROM TO -> /suspect reports a member would step with a member-side damper: one per
(colo, about, term) per GAP ms, every verdict, the rest answered at once from the last verdict. From `Reported` (28)
rows, which log only reports that reached `verdict`, and `Entered` (25) rows for every /suspect that entered.
  python3 tests/fanout/votershed/damp.py /tmp/shed/rows.pkl 2026-11-18T01:00:18Z 2026-11-18T01:05:18Z"""
import collections, datetime, pickle, sys
rows = pickle.load(open(sys.argv[1], 'rb'))
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp() * 1000)
T0, T1 = iso(sys.argv[2]), iso(sys.argv[3])
secs = (T1 - T0) / 1000
for s in sorted(rows):
    rep = sorted(x for x in rows[s] if x[1] == 28 and T0 <= x[0] <= T1)
    entered = sum(1 for x in rows[s] if x[1] == 25 and x[4] % 64 == 7 and T0 <= x[0] <= T1)
    out = [f"member {s}: /suspect entered {entered / secs:.0f}/s, stepped {len(rep) / secs:.0f}/s"]
    for gap in (1000, 5000, 10000):
        last, n = {}, 0
        for x in rep:
            k = (x[4], x[2], x[3])
            if k not in last or x[0] - last[k] >= gap:
                n += 1
                last[k] = x[0]
        out.append(f"gap {gap} ms -> {n / secs:.1f}/s")
    keys = len({(x[4], x[2], x[3]) for x in rep})
    print("; ".join(out) + f"; distinct (colo, about, term) {keys}")
