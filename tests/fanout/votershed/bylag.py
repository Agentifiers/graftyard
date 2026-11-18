#!/usr/bin/env python3
"""bylag.py ROWS.pkl FROM TO TERMS -> per leader-second, each inbound stream's count against the ticker-stall ms k
seconds later (k < 0: the stream follows the stall). A stream that causes stalls leads them; one the stalls cause
(the receipt's ask after COMMIT_RECEIPT, `/outcome` after COMMIT_HOLD) follows them. Also `/raft` by caller class.
  uv run -q --with numpy --with scipy python3 tests/fanout/votershed/bylag.py /tmp/m50/rows.pkl FROM TO TERMS"""
import bisect, collections, datetime, json, pickle, sys
from scipy.stats import spearmanr
P, F, T, TERMS = sys.argv[1:5]
rows = pickle.load(open(P, 'rb'))
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp() * 1000)
T0, T1 = iso(F), iso(T)
R = ["/raft", "/read", "/commit", "/commit_many", "/outcome", "/outcome_by_request", "/suspect"]
route = lambda c: R[c - 1] if 0 < c <= 7 else 'other'
STEP = {0: 'other', 1: 'leader', 2: 'walk', 3: 'spread', 4: 'send', 5: 'ask'}
samples = [(iso(r['at']), {m[0]: m[1] for m in r['members']}) for r in map(json.loads, open(TERMS)) if r.get('members')]
st = [t for t, _ in samples]
leader = lambda t: next((k for k, v in samples[max(0, bisect.bisect_right(st, t) - 1)][1].items() if v == 'leader'), None)
for s in sorted(rows):
    V = rows[s]
    E = [x for x in V if x[1] == 25 and T0 <= x[0] <= T1 and leader(x[0]) == s]
    if len(E) < 5000:
        continue
    sb = {x[3]: x[5] for x in V if x[1] == 27}
    def key(x):
        r = route(x[4] % 64)
        if r == '/raft':
            return '/raft:learner' if x[2] >= 192 else '/raft:voter'
        return f"{r}:{STEP.get(sb.get(x[4] // 64), '-')}"
    by = collections.defaultdict(collections.Counter)
    for x in E:
        by[x[0] // 1000][key(x)] += 1
    stall = collections.Counter()
    for x in V:
        if x[1] == 8 and T0 <= x[0] <= T1:
            a, b = x[0] - x[5], x[0]
            for sec in range(a // 1000, b // 1000 + 1):
                stall[sec] += max(0, min(b, (sec + 1) * 1000) - max(a, sec * 1000))
    secs = sorted(by)
    tot = sum(by.values(), collections.Counter())
    print(f"member {s}: {len(secs)} leader-seconds; spearman(stream count at t, stall ms at t+k)")
    print(f"  {'stream':28} {'/s':>7}  " + "  ".join(f"k={k:+d}" for k in (-2, -1, 0, 1, 2)))
    for k_, v in tot.most_common(8):
        cs = []
        for k in (-2, -1, 0, 1, 2):
            p = [(by[b][k_], stall[b + k]) for b in secs if (b + k) in by]
            cs.append(f"{spearmanr([a for a, _ in p], [c for _, c in p])[0]:5.2f}")
        print(f"  {k_:28} {v / len(secs):7.1f}  " + "  ".join(cs))
