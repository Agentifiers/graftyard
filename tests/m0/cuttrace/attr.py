"""B0-29 commit tail, per command from the leader's trace (one clock: the isolate's raw).
Each /commit arrival (Entered 25, route 3) is matched to the entry that applied first at or after it and whose append
came after the arrival (Stepped 15: `index` the last entry queued; Applied 4: `index` applied). Lower bound on its
commit; split:
  slot  arrival -> apply of the entry before its own (the entry in flight)
  post  that apply -> its own entry's append (cut, deps, decide)
  round its own append -> its apply (confirm + quorum)
Shares for all commands and for those at or over the modelled p99, the tail's own round against its entry's bytes
and the far voter, and per-window p99 (30 s). Usage: attr.py TSAVE FROM TO"""
import bisect, collections, datetime, os, sys
sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..', 'fanout')))
from tracerows import load
P, F, T = sys.argv[1:4]
SKIP = int(sys.argv[4]) if len(sys.argv) > 4 else 0
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
T0, T1 = iso(F), iso(T)
arr = []; app = {}; aply = []; q12 = {}; byts = collections.defaultdict(int); ended = collections.defaultdict(dict)
conf = []
for r in load(P, keep=lambda r: T0 - 10000 <= r[0] < T1 + 10000).get(0, []):
    if r[1] == 25 and r[4] % 64 == 3 and T0 <= r[0] < T1: arr.append(r[0])
    elif r[1] == 15: app.setdefault(r[4], r[0])
    elif r[1] == 4: aply.append((r[0], r[4]))
    elif r[1] == 12 and r[2] in (1, 2):
        q12[(r[2], r[3])] = r[4]; byts[r[4]] = max(byts[r[4]], r[5])
    elif r[1] == 14 and r[2] in (1, 2) and r[4] == 0 and (r[2], r[3]) in q12:
        ended[q12[(r[2], r[3])]].setdefault(r[2], r[0])
    elif r[1] == 10: conf.append((r[0], r[4], r[5]))
aply.sort(); applied_at = {}; prev = 0
for at, idx in aply:
    for i in range(prev + 1, idx + 1): applied_at.setdefault(i, at)
    prev = max(prev, idx)
ents = sorted((at, i) for i, at in app.items() if i in applied_at)
eat = [a for a, _ in ents]
pc = lambda xs, q: sorted(xs)[min(len(xs) - 1, int(len(xs) * q))] if xs else None
rows = []
for e in sorted(arr):
    j = bisect.bisect_right(eat, e) + SKIP  # SKIP 1: read-ahead, it rides the second entry appended after arrival
    if j >= len(ents): continue
    a, i = ents[j]
    prev_apply = applied_at.get(ents[j - 1][1]) if j > 0 else None
    freed = min(max(prev_apply or e, e), a)
    slot, post, rnd = freed - e, a - freed, applied_at[i] - a
    rows.append((slot + post + rnd, slot, post, rnd, i))
tot = [r[0] for r in rows]
p99 = pc(tot, .99); tail = [r for r in rows if r[0] >= p99]
print(f"commands {len(rows)}; commit (lower bound) p50 {pc(tot,.5)} p90 {pc(tot,.9)} p99 {p99}; entries {len(ents)}")
for k, n in ((1, "slot (entry in flight)"), (2, "post (cut, deps, decide)"), (3, "own round")):
    print(f"  {n:26}: p50 {pc([r[k] for r in rows],.5)} p99 {pc([r[k] for r in rows],.99)}; mean share {100*sum(r[k] for r in rows)/sum(tot):.0f}%, tail share {100*sum(r[k] for r in tail)/sum(r[0] for r in tail):.0f}%, tail mean {sum(r[k] for r in tail)/len(tail):.0f}")
# The tail's own entries and the entries in flight ahead of them
te = sorted({r[4] for r in tail})
rr = {i: applied_at[i] - a for a, i in ents}
allr = list(rr.values())
print(f"round all entries p50 {pc(allr,.5)} p90 {pc(allr,.9)} p99 {pc(allr,.99)}; tail's own entries ({len(te)}) round p50 {pc([rr[i] for i in te],.5)} max {max(rr[i] for i in te)}")
idx = [i for _, i in ents]
ahead = sorted({idx[idx.index(i) - 1] for i in te if idx.index(i) > 0})
print(f"entries in flight ahead of the tail ({len(ahead)}): round p50 {pc([rr[i] for i in ahead],.5)} max {max(rr[i] for i in ahead) if ahead else None}")
# far voter and bytes
far = lambda i: ended.get(i, {}).get(2, 1e18) < ended.get(i, {}).get(1, 1e18)
print(f"quorum by far voter: all {sum(far(i) for i in rr)}/{len(rr)}; tail own {sum(far(i) for i in te)}/{len(te)}; ahead {sum(far(i) for i in ahead)}/{len(ahead)}")
leg = lambda i, p: (ended.get(i, {}).get(p) - app[i]) if ended.get(i, {}).get(p) else None
for p in (1, 2):
    xs = [leg(i, p) for i in rr if leg(i, p) is not None]; tx = [leg(i, p) for i in set(te) | set(ahead) if leg(i, p) is not None]
    print(f"  site {p} append leg: all p50 {pc(xs,.5)} p99 {pc(xs,.99)}; tail+ahead p50 {pc(tx,.5)} max {max(tx) if tx else None}")
bb = [byts[i] for i in rr]; tb = [byts[i] for i in set(te) | set(ahead)]
print(f"entry bytes: all p50 {pc(bb,.5)} p99 {pc(bb,.99)}; tail+ahead p50 {pc(tb,.5)} p90 {pc(tb,.9)}")
# Leader confirm in the tail's entries: confirm rows covering index i (the first confirm with index >= i)
ci = sorted((ix, at, ms) for at, ix, ms in conf)
cix = [c[0] for c in ci]
def conf_for(i):
    k = bisect.bisect_left(cix, i)
    return ci[k][2] if k < len(ci) else None
print(f"confirm: all entries p50 {pc([c for c in (conf_for(i) for i in rr) if c is not None],.5)}; tail+ahead p50 {pc([c for c in (conf_for(i) for i in set(te)|set(ahead)) if c is not None],.5)} max {max([c for c in (conf_for(i) for i in set(te)|set(ahead)) if c is not None] or [None])}")
# time clustering of the tail
w = collections.Counter((e - T0) // 30000 for e in [r for r in sorted(arr)][:0])
tw = collections.Counter()
for e in sorted(arr):
    pass
tails_at = []
for e, r in zip(sorted(arr), rows):
    pass
# recompute with arrival kept
rows2 = []
for e in sorted(arr):
    j = bisect.bisect_right(eat, e) + SKIP
    if j >= len(ents): continue
    a, i = ents[j]; rows2.append((e, applied_at[i] - e))
c30 = collections.Counter((e - T0) // 30000 for e, c in rows2 if c >= p99)
print("tail commands by 30 s window from load start:", dict(sorted(c30.items())))
