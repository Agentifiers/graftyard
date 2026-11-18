#!/usr/bin/env python3
"""bystep.py ROWS.pkl FROM TO TERMS -> the leader's inbound requests by route and lookup step (SentBy), beside its
ticker-stall share and the leader-to-follower out leg, per second while it led. Which inbound stream to cut first
(tests/am1/results/c-design.md, "(c1b) under review"). ROWS.pkl as prep.py writes it, from any tsave.
  uv run -q --with numpy --with scipy python3 tests/fanout/votershed/bystep.py /tmp/m50/rows.pkl FROM TO TERMS"""
import bisect, collections, datetime, json, pickle, sys
import numpy as np
from scipy.stats import spearmanr

P, F, T, TERMS = sys.argv[1:5]
rows = pickle.load(open(P, 'rb'))
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp() * 1000)
T0, T1 = iso(F), iso(T)
pct = lambda xs, q: (sorted(xs)[min(len(xs) - 1, int(q * len(xs)))] if xs else None)
R = ["/raft", "/read", "/commit", "/commit_many", "/outcome", "/outcome_by_request", "/suspect"]
route = lambda c: R[c - 1] if 0 < c <= 7 else 'other'
STEP = {0: 'other', 1: 'leader', 2: 'walk', 3: 'spread', 4: 'send', 5: 'ask'}
samples = []
for l in open(TERMS):
    r = json.loads(l)
    if r.get('members'):
        samples.append((iso(r['at']), {m[0]: m[1] for m in r['members']}))
st = [t for t, _ in samples]


def leader(t):
    i = max(0, bisect.bisect_right(st, t) - 1)
    return next((k for k, v in samples[i][1].items() if v == 'leader'), None)


# Pair skew from voter-to-voter /raft calls (outleg.py's method).
ent = {s: [x for x in v if x[1] == 25] for s, v in rows.items()}
raft_in = {}
for s in ent:
    for x in ent[s]:
        if x[4] % 64 == 1 and x[2] < 128 and x[3]:
            raft_in[(x[2], x[3])] = (s, x[0])
left = {(x[2], x[3]): x[0] for s in rows for x in rows[s] if x[1] == 24}
ans = {(s, x[3]): x[0] for s in rows for x in rows[s] if x[1] == 21 and x[3]}
outs, backs = collections.defaultdict(list), collections.defaultdict(list)
for s in rows:
    for x in rows[s]:
        if x[1] == 23 and (s, x[3]) in raft_in:
            c, at = raft_in[(s, x[3])]
            outs[(s, c)].append(at - x[0])
            if (s, x[3]) in ans and (s, x[3]) in left:
                backs[(s, c)].append(ans[(s, x[3])] - left[(s, x[3])])
skew = {p: (min(outs[p]) - min(backs[p])) // 2 for p in outs if backs[p]}

total = collections.Counter()
secs_all = 0
per_sec = []  # (requests, stall ms, out-leg median or None, per-stream counts)
for s in sorted(rows):
    V = rows[s]
    E = [x for x in ent[s] if T0 <= x[0] <= T1 and leader(x[0]) == s]
    if len(E) < 1000:
        continue
    sb = {x[3]: x[5] for x in V if x[1] == 27}
    key = lambda x: f"{route(x[4] % 64)}:{STEP.get(sb.get(x[4] // 64), '-') if x[2] == 255 else 'peer'}"
    secs = sorted({x[0] // 1000 for x in E})
    secs_all += len(secs)
    c = collections.Counter(key(x) for x in E)
    total += c
    stall = collections.Counter()
    for x in V:
        if x[1] == 8 and T0 <= x[0] <= T1 and leader(x[0]) == s:
            a, b = x[0] - x[5], x[0]
            for sec in range(a // 1000, b // 1000 + 1):
                stall[sec] += max(0, min(b, (sec + 1) * 1000) - max(a, sec * 1000))
    out = collections.defaultdict(list)
    for x in V:
        if x[1] == 23 and T0 <= x[0] <= T1 and (s, x[3]) in raft_in and leader(x[0]) == s and (s, x[2]) in skew:
            cc, at = raft_in[(s, x[3])]
            out[x[0] // 1000].append(at - x[0] - skew[(s, x[2])])
    bysec = collections.defaultdict(collections.Counter)
    for x in E:
        bysec[x[0] // 1000][key(x)] += 1
    for b in secs:
        per_sec.append((sum(bysec[b].values()), stall[b], float(np.median(out[b])) if out.get(b) else None, bysec[b]))
    print(f"member {s}: led {len(secs)} s in the window, {len(E) / len(secs):.0f} req/s, "
          f"stall {sum(stall[b] for b in secs) / 1000:.1f} s ({sum(stall[b] for b in secs) / len(secs) / 10:.0f}%)")

print(f"\nleader inbound by route:step over {secs_all} leader-seconds (req/s, share)")
n = sum(total.values())
for k, v in total.most_common(16):
    print(f"  {k:30} {v / secs_all:7.1f}/s  {v / n:5.1%}")

print("\nstall share of the second by requests entered that second")
for lo, hi in ((0, 300), (300, 600), (600, 900), (900, 1200), (1200, 10 ** 9)):
    v = [p[1] for p in per_sec if lo <= p[0] < hi]
    o = [p[2] for p in per_sec if lo <= p[0] < hi and p[2] is not None]
    if v:
        print(f"  {lo:>5}-{hi:<6} s {len(v):4}  stall {np.mean(v) / 1000:5.0%}  out leg p50 {pct(o, .5)} p90 {pct(o, .9)}")

w = [p for p in per_sec if p[2] is not None]
print(f"\nper second, n {len(per_sec)} ({len(w)} with an out leg): spearman with stall ms, with the out leg")
print(f"  {'all requests':30} {spearmanr([p[0] for p in per_sec], [p[1] for p in per_sec])[0]:5.2f}  "
      f"{spearmanr([p[0] for p in w], [p[2] for p in w])[0]:5.2f}")
for k, _ in total.most_common(10):
    print(f"  {k:30} {spearmanr([p[3][k] for p in per_sec], [p[1] for p in per_sec])[0]:5.2f}  "
          f"{spearmanr([p[3][k] for p in w], [p[2] for p in w])[0]:5.2f}")
# Each stream's share of the requests a second where the stall share was 30% or more.
hot = [p for p in per_sec if p[1] >= 300]
hc = sum((p[3] for p in hot), collections.Counter())
hn = sum(hc.values())
if hn:
    print(f"\nin the {len(hot)} leader-seconds with 30%+ stall: requests by stream (share there vs overall)")
    for k, v in hc.most_common(10):
        print(f"  {k:30} {v / len(hot):7.1f}/s  {v / hn:5.1%} vs {total[k] / n:5.1%}")
