"""B0-29 commit per command, matched through the leader's cuts (one raw clock). A /commit arrival (Entered 25, route 3)
rides the batch of the first cut (Cut 29) at or after it; that cut's entry is the first append (Stepped 15) at or
after the cut, and the command commits at that entry's apply (Applied 4). Split:
  cutwait  arrival -> its cut           (waiting for the next freeze)
  held     cut -> previous entry applied (the frozen batch waits for the entry in flight)
  post     that apply -> own append      (deps read and decide past the apply)
  round    own append -> own apply
Prints the modelled commit against /stats, shares of mean and of the tail (>= p99), and what the tail's rounds had.
Usage: bycut.py TSAVE FROM TO"""
import bisect, collections, datetime, json, lzma, sys
P, F, T = sys.argv[1:4]
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
T0, T1 = iso(F), iso(T)
seen = set(); arr = []; app = {}; aply = []; cuts = []; q12 = {}; ended = collections.defaultdict(dict); byts = collections.defaultdict(int)
with (lzma.open if P.endswith(".xz") else open)(P, "rt") as f:
    for l in f:
        d = json.loads(l)
        if "rows" not in d or d.get("learner") is not None or d.get("site") != 0: continue
        for r in d["rows"]:
            t = tuple(r)
            if t in seen: continue
            seen.add(t)
            if not (T0 - 10000 <= r[0] < T1 + 10000): continue
            k = r[1]
            if k == 25 and r[4] % 64 == 3 and T0 <= r[0] < T1: arr.append(r[0])
            elif k == 15: app.setdefault(r[4], r[0])
            elif k == 4: aply.append((r[0], r[4]))
            elif k == 29: cuts.append(r[0])
            elif k == 12 and r[2] in (1, 2): q12[(r[2], r[3])] = r[4]; byts[r[4]] = max(byts[r[4]], r[5])
            elif k == 14 and r[2] in (1, 2) and r[4] == 0 and (r[2], r[3]) in q12: ended[q12[(r[2], r[3])]].setdefault(r[2], r[0])
aply.sort(); applied_at = {}; prev = 0
for at, idx in aply:
    for i in range(prev + 1, idx + 1): applied_at.setdefault(i, at)
    prev = max(prev, idx)
ents = sorted((at, i) for i, at in app.items() if i in applied_at)
eat = [a for a, _ in ents]; cuts.sort()
pc = lambda xs, q: sorted(xs)[min(len(xs) - 1, int(len(xs) * q))] if xs else None
rows = []
for e in sorted(arr):
    c = bisect.bisect_left(cuts, e)
    if c >= len(cuts): continue
    ct = cuts[c]
    j = bisect.bisect_left(eat, ct)
    if j >= len(ents) or j == 0: continue
    a, i = ents[j]; pa = applied_at[ents[j - 1][1]]
    held = max(0, min(pa, a) - ct); post = a - max(ct, min(pa, a))
    rows.append((applied_at[i] - e, ct - e, held, post, applied_at[i] - a, i, ents[j - 1][1], e))
tot = [r[0] for r in rows]; p99 = pc(tot, .99); tail = [r for r in rows if r[0] >= p99]
print(f"commands {len(rows)} of {len(arr)}; modelled commit p50 {pc(tot,.5)} p90 {pc(tot,.9)} p99 {p99}; cuts {len(cuts)}, entries {len(ents)}")
for k, n in ((1, "cutwait (to its freeze)"), (2, "held (entry in flight)"), (3, "post (deps, decide)"), (4, "own round")):
    v = [r[k] for r in rows]; tv = [r[k] for r in tail]
    print(f"  {n:24}: p50 {pc(v,.5):4} p99 {pc(v,.99):4} | mean share {100*sum(v)/sum(tot):3.0f}% | tail share {100*sum(tv)/sum(r[0] for r in tail):3.0f}%, tail mean {sum(tv)/len(tv):.0f}")
rr = {i: applied_at[i] - a for a, i in ents}
own = sorted({r[5] for r in tail}); ahead = sorted({r[6] for r in tail})
leg = lambda i, p: (ended[i][p] - app[i]) if p in ended.get(i, {}) else None
for name, ids in (("all entries", list(rr)), ("tail's own", own), ("in flight ahead of tail", ahead)):
    print(f"  {name:24} n {len(ids):4}: round p50 {pc([rr[i] for i in ids],.5)} p99 {pc([rr[i] for i in ids],.99)}; "
          f"near leg p50 {pc([x for x in (leg(i,1) for i in ids) if x],.5)} | bytes p50 {pc([byts[i] for i in ids],.5)} p90 {pc([byts[i] for i in ids],.9)}")
# Two-round floor: round(in flight) + round(own), against the modelled commit
two = [rr[r[6]] + r[4] for r in rows]
print(f"two rounds (ahead + own): p50 {pc(two,.5)} p99 {pc(two,.99)}; commit minus two rounds p50 {pc([r[0]-t for r,t in zip(rows,two)],.5)} p99 {pc([r[0]-t for r,t in zip(rows,two)],.99)}")
print("tail by 30 s window:", dict(sorted(collections.Counter((r[7] - T0) // 30000 for r in tail).items())))
# Upper bound for a late join: a command arriving after a batch froze but before that batch's entry is appended joins
# it (only if its deps need no new read, which the trace cannot tell, so this bounds every command).
late = []
for r in rows:
    e = r[7]
    c = bisect.bisect_left(cuts, e)
    pc_ = cuts[c - 1] if c > 0 else None
    if pc_ is not None:
        j = bisect.bisect_left(eat, pc_)
        if j < len(ents) and ents[j][0] >= e:
            late.append(applied_at[ents[j][1]] - e); continue
    late.append(r[0])
print(f"late join, every command (upper bound): commit p50 {pc(late,.5)} p99 {pc(late,.99)}; joinable share {sum(1 for a,b in zip(late,tot) if a<b)/len(tot):.0%}")
