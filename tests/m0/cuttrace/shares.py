import os, sys, bisect, collections
sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..', 'fanout')))
from tracerows import load
# Per /commit arrival on the leader, under read-ahead's freeze (the next batch freezes W ms after the entry in flight
# was appended; `cut_early`), one clock (raw): the command rides entry j+1, the batch frozen during entry j's flight.
#   a: arrival -> freeze               (waiting for the next freeze)
#   b: freeze -> entry j applies       (the frozen batch waits for the entry in flight)
#   c: entry j applies -> j+1 appended (decide waits for its deps read, the commit-to-next-cut gap)
#   d: j+1 appended -> j+1 applies     (its own round)
# Shares of the mean, and of the commands over the p99 (the tail). Also which voter's ack made each round's quorum
# (AppendEnded 14 per peer, by round), to give append:2's far leg its share.
path, t0, W = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]); t1 = t0 + 600000
arr = []; app = {}; aply = []; ended = collections.defaultdict(dict); q12 = {}
for r in load(path, keep=lambda r: t0 - 5000 <= r[0] < t1 + 5000).get(0, []):
    if r[1] == 25 and r[4] % 64 == 3 and t0 <= r[0] < t1: arr.append(r[0])
    if r[1] == 15: app.setdefault(r[4], r[0])
    if r[1] == 4: aply.append((r[0], r[4]))
    if r[1] == 12 and r[2] in (1, 2): q12[(r[2], r[3])] = r[4]
    if r[1] == 14 and r[2] in (1, 2) and r[4] == 0 and (r[2], r[3]) in q12:
        ended[q12[(r[2], r[3])]].setdefault(r[2], r[0])
aply.sort(); applied_at = {}; prev = 0
for at, idx in aply:
    for i in range(prev + 1, idx + 1): applied_at.setdefault(i, at)
    prev = max(prev, idx)
ents = sorted((at, i) for i, at in app.items() if i in applied_at)
fz = [a + W for a, _ in ents]
rows = []
for e in sorted(arr):
    j = bisect.bisect_right(fz, e)
    if j + 1 >= len(ents): continue
    (aj, ij), (a1, i1) = ents[j], ents[j + 1]
    p = (fz[j] - e, max(0, applied_at[ij] - fz[j]), max(0, a1 - applied_at[ij]), applied_at[i1] - a1)
    rows.append((sum(p), p, i1))
def pc(xs, q): xs = sorted(xs); return xs[min(len(xs) - 1, int(len(xs) * q))]
tot = [r[0] for r in rows]
print(f"W {W}: commands {len(rows)}; modelled commit p50 {pc(tot,.5)} p99 {pc(tot,.99)}")
names = ['a wait for freeze', 'b frozen, entry in flight', 'c apply->next cut (deps/decide)', 'd own round']
p99 = pc(tot, .99); tail = [r for r in rows if r[0] >= p99]
for k, n in enumerate(names):
    print(f"  {n}: mean share {100*sum(r[1][k] for r in rows)/sum(tot):.0f}%, tail share {100*sum(r[1][k] for r in tail)/sum(r[0] for r in tail):.0f}%, p99 {pc([r[1][k] for r in rows],.99)}")
# Quorum maker per entry: the voter whose ok AppendEnded came first (leader + one = quorum of three).
first = collections.Counter(); far_extra = []
for i, ends in ended.items():
    if 1 in ends and 2 in ends:
        first[1 if ends[1] <= ends[2] else 2] += 1
    elif ends: first[next(iter(ends))] += 1
print(f"quorum made by site 1 (near) {first[1]}, site 2 (far) {first[2]} entries")
tail_ents = collections.Counter(r[2] for r in tail)
fm = sum(1 for i in tail_ents if ended.get(i, {}).get(2, 1e18) < ended.get(i, {}).get(1, 1e18))
print(f"tail commands' own entries whose quorum the far voter made: {fm} of {len(tail_ents)}")
