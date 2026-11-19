"""Idle leader confirm per trace (before any learner /read load): each voter's confirms in the first 3 minutes of the trace.
Usage: pre.py TSAVE..."""
import json, sys, collections, lzma, datetime
pct = lambda xs, q: (sorted(xs)[min(len(xs) - 1, int(q * len(xs)))] if xs else None)
for P in sys.argv[1:]:
    op = lzma.open if P.endswith(".xz") else open
    rows = collections.defaultdict(set); lead = {}
    for l in op(P, "rt"):
        d = json.loads(l)
        if "rows" in d and d.get("learner") is None:
            rows[d.get("site")].update(tuple(x) for x in d["rows"] if x[1] in (10, 25))
            lead[d.get("site")] = d.get("leader")
    t0 = min(x[0] for v in rows.values() for x in v)
    out = []
    for s in sorted(rows, key=lambda s: (s is None, s)):
        c = [x[5] for x in rows[s] if x[1] == 10 and x[0] < t0 + 180000]
        out.append(f"site {s}{'*' if lead.get(s) == s else ''} {pct(c,.5)}/{pct(c,.99)} n{len(c)}")
    print(f"{P.split('/')[-1]:30} {datetime.datetime.fromtimestamp(t0/1000, datetime.timezone.utc):%m-%d %H:%M}  " + " | ".join(out))
