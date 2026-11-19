#!/usr/bin/env python3
"""leadtail.py TSAVE FROM TO TERMS -> what sits around the acting leader's 1-3 s tail (B0-33 m53's plan, code-only).

1. The leader's `/raft` round trips (CallSent 23 to AnswerArrived 21) at 1 s or more against faster ones: the out-leg
   (send to the callee's handler entry, CallLeft 24's `index`) and the way back, each corrected by the pair's clock
   offset (half the gap between the 5th percentiles of the two legs), the callee's handler time, the leader's own
   storage confirm (Confirm 10) still open at the send, which the output gate holds the fetch behind, and the
   callee's confirm open when its handler returned.
2. The leader's stall seconds (a tick lag, Lag 8, or a confirm ending that second, at 500 ms or more) against the
   whole-tier reads (`packed`, ReadQuery 26 code 22) and other inbound (Entered 25) in that second and the 3 before,
   and where the leader's `packed`, `core` (code 24, the read gate's rows) and `lsRefs` (code 25) reads came from
   (SentBy 27's lookup step). Codes are `READ_QUERIES` index + 1 (`workers/site/src/host.rs`).
Learners' rows are left out. Times are on Cloudflare's clock (the trace's own stamps)."""
import bisect, collections, datetime, json, lzma, sys

P, F, T, TERMS = sys.argv[1:5]
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
T0, T1 = iso(F), iso(T)
rows = collections.defaultdict(set)
for line in (lzma.open if P.endswith(".xz") else open)(P, "rt"):
    d = json.loads(line)
    if "rows" in d and d.get("learner") is None:
        rows[d["site"]].update(tuple(x) for x in d["rows"] if x[1] in (8, 10, 21, 23, 24, 25, 26, 27))
S = [(iso(d["at"]), {m[0]: m[1] for m in d["members"]}) for d in map(json.loads, open(TERMS)) if d.get("members")]
st = [t for t, _ in S]
role = lambda t, s: S[max(0, bisect.bisect_right(st, t) - 1)][1].get(s)
pc = lambda v, p: sorted(v)[min(len(v) - 1, int(p * len(v)))] if v else None
lead = lambda x, s: T0 <= x[0] < T1 and role(x[0], s) == "leader"

left = {(s, x[3]): (x[4], x[0]) for s, V in rows.items() for x in V if x[1] == 24}
conf = {s: sorted((x[0] - x[5], x[0]) for x in V if x[1] == 10) for s, V in rows.items()}
open_at = lambda s, t: max([e - t for b, e in conf.get(s, []) if b <= t <= e] + [0])
trips = collections.defaultdict(list)
unmatched = 0
for L, V in rows.items():
    ans = {x[3]: x[0] for x in V if x[1] == 21}
    for x in V:
        if x[1] != 23 or x[2] not in (0, 1, 2) or not lead(x, L):
            continue
        a, l = ans.get(x[3]), left.get((x[2], x[3]))
        if not a or not l:
            unmatched += 1
            continue
        trips[(L, x[2])].append((a - x[0], l[0] - x[0], l[1] - l[0], a - l[1], open_at(L, x[0]), open_at(x[2], l[1])))
print(f"1. leader /raft round trips, matched {sum(map(len, trips.values()))}, unmatched {unmatched}")
agg = collections.defaultdict(list)
for k, v in trips.items():
    o, b = sorted(t[1] for t in v), sorted(t[3] for t in v)
    th = (o[len(o) // 20] - b[len(b) // 20]) / 2
    for t in v:
        g = "slow" if t[0] >= 1000 else "fast"
        for n, val in zip(("rtt", "out", "handler", "back", "leaderConfirmOpenAtSend", "calleeConfirmOpenAtReturn"),
                          (t[0], t[1] - th, t[2], t[3] + th, t[4], t[5])):
            agg[(g, n)].append(val)
for g in ("fast", "slow"):
    for n in ("rtt", "out", "handler", "back", "leaderConfirmOpenAtSend", "calleeConfirmOpenAtReturn"):
        v = agg[(g, n)]
        print(f"  {g} {n:26} n {len(v):5}  p50 {pc(v, .5)}  p90 {pc(v, .9)}  >=500 ms {sum(x >= 500 for x in v)}")

sec = collections.defaultdict(collections.Counter)
src = collections.Counter()
for s, V in rows.items():
    by = {x[3]: x[5] for x in V if x[1] == 27}
    for x in V:
        if not lead(x, s):
            continue
        k = (s, x[0] // 1000)
        sec[k]["seen"] = 1
        if x[1] == 26 and x[4] in (22, 24, 25):
            sec[k]["tier"] += x[4] == 22
            src[({22: "packed", 24: "core", 25: "lsRefs"}[x[4]], "index" if x[5] else "leader",
                 {0: "other", 1: "Leader", 2: "Walk", 3: "Spread"}.get(by.get(x[3]), "none"))] += 1
        elif x[1] == 25:
            sec[k]["in"] += 1
        elif x[1] in (8, 10):
            sec[k]["stall"] = max(sec[k]["stall"], x[5])
prior = lambda k, f: sum(sec[(k[0], k[1] - i)][f] for i in range(4) if (k[0], k[1] - i) in sec)
keys = list(sec)
stalled = lambda k: sec[k]["stall"] >= 500
print(f"2. leader-seconds {len(keys)}, stall seconds {sum(map(stalled, keys))}")
for name, f, bands in (("whole-tier reads (4 s)", lambda k: prior(k, "tier"), (0, 50, 200, 500)),
                       ("other inbound (4 s)", lambda k: prior(k, "in") - prior(k, "tier"), (0, 200, 600, 1500))):
    for lo, hi in zip(bands, bands[1:] + (10**9,)):
        K = [k for k in keys if lo <= f(k) < hi]
        print(f"  {name} {lo}-{hi if hi < 10**9 else ''}: seconds {len(K)}, stalled {sum(map(stalled, K))}"
              f" ({100 * sum(map(stalled, K)) / max(1, len(K)):.0f}%)")
print("  packed, core and lsRefs reads at the leader by (query, kind, lookup step):")
for k, v in src.most_common():
    print(f"    {k}: {v}")
