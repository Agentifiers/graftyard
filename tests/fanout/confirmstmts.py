#!/usr/bin/env python3
"""confirmstmts.py TSAVE FROM TO TERMS -> the acting leader's storage confirms (`Confirm` 10, `ms`) by the statements
of the transaction each confirms (`Flushed` 16, `index`), for B0-33's "confirm p50/p99 by statements per flush".

A confirm starts in `Host::flush` right after its transaction commits, so its start (`at` - `ms`) falls at or just
after the `Flushed` row of the deferred flush that ran it; a confirm whose start has no `Flushed` within PAIR_MS before
it came from a flush run in the step itself (no `Flushed` row) and is counted as `unpaired`. Learners left out; times
are the trace's own (Cloudflare's clock)."""
import bisect, collections, datetime, json, lzma, sys

PAIR_MS = 5
BANDS = ((0, 50), (50, 200), (200, 500), (500, 900), (900, 10**9))
P, F, T, TERMS = sys.argv[1:5]
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
T0, T1 = iso(F), iso(T)
rows = collections.defaultdict(set)
for line in (lzma.open if P.endswith(".xz") else open)(P, "rt"):
    d = json.loads(line)
    if "rows" in d and d.get("learner") is None:
        rows[d["site"]].update(tuple(x) for x in d["rows"] if x[1] in (10, 16))
S = [(iso(d["at"]), {m[0]: m[1] for m in d["members"]}) for d in map(json.loads, open(TERMS)) if d.get("members")]
st = [t for t, _ in S]
role = lambda t, s: S[max(0, bisect.bisect_right(st, t) - 1)][1].get(s)
pc = lambda v, p: sorted(v)[min(len(v) - 1, int(p * len(v)))] if v else None
by = collections.defaultdict(list)
for s, V in rows.items():
    fl = sorted((x[0], x[4]) for x in V if x[1] == 16)
    ft = [t for t, _ in fl]
    for x in V:
        if x[1] != 10 or not T0 <= x[0] < T1 or role(x[0], s) != "leader":
            continue
        start = x[0] - x[5]
        i = bisect.bisect_right(ft, start) - 1
        n = fl[i][1] if i >= 0 and start - ft[i] <= PAIR_MS else None
        band = "unpaired" if n is None else next(f"{lo}-{hi - 1 if hi < 10**9 else ''}" for lo, hi in BANDS if lo <= n < hi)
        by[band].append(x[5])
        by["all"].append(x[5])
for k in ["all", "unpaired"] + [f"{lo}-{hi - 1 if hi < 10**9 else ''}" for lo, hi in BANDS]:
    v = by.get(k, [])
    print(f"  stmts {k:9} confirms {len(v):5}  p50 {pc(v, .5)}  p99 {pc(v, .99)}  >=1 s {sum(m >= 1000 for m in v)}")
