#!/usr/bin/env python3
"""removed.py ROWS.pkl FROM TO [B] -> per member, the inbound per second the voter shed would answer at the handler top
with bound B (64): edge (`from` 255) /read and /outcome_by_request that entered at or past B open, and every /suspect
repeat of a (colo, about, term) whose last stepped verdict was `promised` less than 1 s before (from `Reported` rows;
an entered /suspect that never stepped is counted as a repeat when its member had stepped any report in that 1 s).
  python3 tests/fanout/votershed/removed.py /tmp/shed/rows.pkl 2026-11-18T01:00:18Z 2026-11-18T01:05:18Z 64"""
import bisect, collections, datetime, pickle, sys
rows = pickle.load(open(sys.argv[1], 'rb'))
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp() * 1000)
T0, T1 = iso(sys.argv[2]), iso(sys.argv[3])
B = int(sys.argv[4]) if len(sys.argv) > 4 else 64
secs = (T1 - T0) / 1000
for s in sorted(rows):
    ent = [x for x in rows[s] if x[1] == 25 and T0 <= x[0] <= T1]
    total = len(ent)
    edge = [x for x in ent if x[2] == 255]
    shed = sum(1 for x in edge if x[4] % 64 in (2, 6) and x[5] - 1 >= B)
    sus_in = sum(1 for x in edge if x[4] % 64 == 7)
    rep = sorted(x[0] for x in rows[s] if x[1] == 28 and T0 <= x[0] <= T1)
    stepped_after = 0
    cached = {}
    for x in sorted(x for x in rows[s] if x[1] == 28 and T0 <= x[0] <= T1):
        k = (x[4], x[2], x[3])
        if k in cached and x[0] < cached[k]:
            continue
        stepped_after += 1
        cached.pop(k, None)
        if x[5] == 4:
            cached[k] = x[0] + 1000
    print(f"member {s}: inbound {total / secs:.0f}/s; edge /read+/outcome_by_request shed at B={B}: {shed / secs:.0f}/s;"
          f" /suspect entered {sus_in / secs:.0f}/s, stepped with the cache at most {stepped_after / secs:.1f}/s;"
          f" removed {(shed + sus_in - stepped_after) / secs:.0f}/s ({(shed + sus_in - stepped_after) / max(1, total):.0%})")
