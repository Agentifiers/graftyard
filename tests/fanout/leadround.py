"""Round per entry on whichever voter led: its append (Stepped 15, `index` the last entry queued) to its apply on that
voter (Applied 4), both on that isolate's clock, as `tests/m0/cuttrace/attr.py`'s "own round" but not tied to site 0.
An entry counts for the site that appended it first (the leader; followers append it later), within the window. Usage: leadround.py TSAVE FROM TO"""
import collections, datetime, json, lzma, sys
P, F, T = sys.argv[1:4]
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
T0, T1 = iso(F), iso(T)
seen = set()
app = collections.defaultdict(dict)
aply = collections.defaultdict(list)
for l in (lzma.open if P.endswith(".xz") else open)(P, "rt"):
    d = json.loads(l)
    if "rows" not in d or d.get("learner") is not None:
        continue
    s = d.get("site")
    for r in d["rows"]:
        t = (s,) + tuple(r)
        if t in seen or not (T0 - 10000 <= r[0] < T1 + 30000):
            continue
        seen.add(t)
        if r[1] == 15:
            app[s].setdefault(r[4], r[0])
        elif r[1] == 4:
            aply[s].append((r[0], r[4]))
pc = lambda xs, q: sorted(xs)[min(len(xs) - 1, int(len(xs) * q))] if xs else None
first = {}
for s, m in app.items():
    for i, a in m.items():
        if i not in first or a < first[i][1]:
            first[i] = (s, a)
every = []
for s in sorted(app, key=str):
    applied_at, prev = {}, 0
    for at, idx in sorted(aply[s]):
        for i in range(prev + 1, idx + 1):
            applied_at.setdefault(i, at)
        prev = max(prev, idx)
    rr = [applied_at[i] - a for i, (s2, a) in first.items()
          if s2 == s and T0 <= a < T1 and i in applied_at and applied_at[i] >= a]
    if rr:
        print(f"site {s}: {len(rr)} entries led, round p50 {pc(rr, .5)} p90 {pc(rr, .9)} p99 {pc(rr, .99)} max {max(rr)}")
    every += rr
print(f"all: {len(every)} entries, round p50 {pc(every, .5)} p90 {pc(every, .9)} p99 {pc(every, .99)}")
