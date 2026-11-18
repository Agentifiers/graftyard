#!/usr/bin/env python3
"""learnerrates.py TSAVE FROM TO [SPLIT_ISO] -> each learner's read-index call time p50/p90 (IndexEnded 2's `ms`: the
call from issue to its answer, tests/nr3/legs.py's "call"), and its inbound /read per second (Entered 25, route 2),
before and after SPLIT_ISO (a leader change). Times on Cloudflare's clock (B0-33 m52 against m51)."""
import collections, datetime, json, lzma, sys

P, F, T = sys.argv[1:4]
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
T0, T1 = iso(F), iso(T)
S = iso(sys.argv[4]) if len(sys.argv) > 4 else T1
pct = lambda xs, q: (sorted(xs)[min(len(xs) - 1, int(q * len(xs)))] if xs else None)
hm = lambda t: datetime.datetime.fromtimestamp(t / 1000, datetime.UTC).strftime("%H:%M:%S")
rows = collections.defaultdict(set)
for line in (lzma.open if P.endswith(".xz") else open)(P, "rt"):
    r = json.loads(line)
    if "rows" in r and "learner" in r:
        rows[r["learner"]].update(tuple(x) for x in r["rows"] if x[1] in (2, 25))
for a, b, name in ((T0, S, "before"), (S, T1, "after")):
    if b <= a:
        continue
    out, allrt = [], []
    for l in sorted(rows):
        v = rows[l]
        rt = [x[5] for x in v if x[1] == 2 and a <= x[0] < b]
        allrt += rt
        reads = sum(1 for x in v if x[1] == 25 and x[4] % 64 == 2 and a <= x[0] < b)
        out.append(f"l{l - 192} call p50 {pct(rt, .5)} p90 {pct(rt, .9)} ms (n {len(rt)}), /read {reads / ((b - a) / 1000):.0f}/s")
    print(f"{name} {hm(a)}..{hm(b)}: all learners call p50 {pct(allrt, .5)} p90 {pct(allrt, .9)} ms (n {len(allrt)})")
    for o in out:
        print("  " + o)
