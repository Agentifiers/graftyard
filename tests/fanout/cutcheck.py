#!/usr/bin/env python3
"""cutcheck.py TSAVE FROM TO TERMS [SHIFT_MS] -> write-cuts' verdict figures for a B0-33 row, per leader.

From each member's trace rows while it led (TERMS gives the role each second):
- its deferred transactions (`Flushed` 16, `index` its statements): count, p50, p99, max, and how many reached 900 and
  1,000 statements. An `ExtendLeases` rewrites every live lease at 3 statements each, so on B0-33 (about 450 live
  leases) it is a transaction of 900 or more; none of the rest reached that on m50 to m52 (`tests/am1/results/tx-mix.md`);
- its storage confirms (`Confirm` 10, `ms`): count, p50, p99, max, and how many took 1 s or more;
- the transactions of 900 or more per minute of leading, the `ExtendLeases` firing rate.
  python3 tests/fanout/cutcheck.py tests/fanout/raw/tsave-m53b33.jsonl 2026-11-18T22:00:00Z 2026-11-18T22:05:00Z \\
    tests/fanout/raw/terms-m53b33.jsonl
SHIFT_MS moves TERMS onto the trace's clock for runs before srvclock (m50 +7310, m51 +7440)."""
import bisect, collections, datetime, json, lzma, sys

P, F, T, TERMS = sys.argv[1:5]
SHIFT = int(sys.argv[5]) if len(sys.argv) > 5 else 0
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
T0, T1 = iso(F), iso(T)
pc = lambda v, p: sorted(v)[min(len(v) - 1, int(p * len(v)))] if v else None

rows = collections.defaultdict(set)
for line in (lzma.open if P.endswith(".xz") else open)(P, "rt"):
    r = json.loads(line)
    if "rows" in r and "learner" not in r:
        rows[r["site"]].update(tuple(x) for x in r["rows"] if x[1] in (10, 16))
samples = [(iso(r["at"]) + SHIFT, {m[0]: m[1] for m in r["members"]}) for r in map(json.loads, open(TERMS)) if r.get("members")]
st = [t for t, _ in samples]


def role(t, s):
    return samples[max(0, bisect.bisect_right(st, t) - 1)][1].get(s, "?")


for s in sorted(rows):
    xs = [x for x in rows[s] if T0 <= x[0] < T1 and role(x[0], s) == "leader"]
    if not xs:
        continue
    secs = sum(1 for t in range(T0 // 1000, T1 // 1000) if role(t * 1000 + 500, s) == "leader")
    fl = [x[4] for x in xs if x[1] == 16]
    cf = [x[5] for x in xs if x[1] == 10]
    big = sum(n >= 900 for n in fl)
    print(f"member {s}: led {secs} s")
    if fl:
        print(f"  transactions {len(fl)}: statements p50 {pc(fl, .5)} p99 {pc(fl, .99)} max {max(fl)}; "
              f">=900 {big}, >=1000 {sum(n >= 1000 for n in fl)}; extends per minute {60 * big / max(secs, 1):.2f}")
    if cf:
        print(f"  confirms {len(cf)}: p50 {pc(cf, .5)} p99 {pc(cf, .99)} max {max(cf)} ms; at 1 s or more {sum(v >= 1000 for v in cf)}")
