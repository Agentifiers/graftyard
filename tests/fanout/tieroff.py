#!/usr/bin/env python3
"""tieroff.py TSAVE FROM TO TERMS -> the `tier-off-leader` replay on a B0-33 or B0-29 trace: every whole-tier
(`packed`, ReadQuery code 22) and `ls-refs` (code 25) read that reached the acting leader, and what the rule does with
it. A read-index read (`ms` 1) the leader now refuses `notLeader` and the edge's pass moves to its next member; a plain
read (`ms` 0) came only from `read_spread`'s `Spread::Leader` after every member asked declined (SentBy 27 step Walk
or Leader), and is now never sent: a 503 with `Retry-After`, or a write's shed retry, for `packed`, and the shard for
`ls-refs`. The plain reads are the upper bound on reads the rule turns into 503s; the same declines on m50 to m52
numbered 0. Also each member's plain whole-tier reads while not leading (the walk's misses). Learners left out."""
import bisect, collections, datetime, json, lzma, sys

P, F, T, TERMS = sys.argv[1:5]
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
T0, T1 = iso(F), iso(T)
rows = collections.defaultdict(set)
for line in (lzma.open if P.endswith(".xz") else open)(P, "rt"):
    d = json.loads(line)
    if "rows" in d and d.get("learner") is None:
        rows[d["site"]].update(tuple(x) for x in d["rows"] if x[1] in (26, 27))
S = [(iso(d["at"]), {m[0]: m[1] for m in d["members"]}) for d in map(json.loads, open(TERMS)) if d.get("members")]
st = [t for t, _ in S]
role = lambda t, s: S[max(0, bisect.bisect_right(st, t) - 1)][1].get(s)
STEP = {0: "other", 1: "Leader", 2: "Walk", 3: "Spread"}
lead, other = collections.Counter(), collections.Counter()
by_term = collections.Counter()
for s, V in rows.items():
    by = {x[3]: x[5] for x in V if x[1] == 27}
    for x in V:
        if x[1] != 26 or x[4] not in (22, 25) or not T0 <= x[0] < T1:
            continue
        k = ("packed" if x[4] == 22 else "lsRefs", "read-index" if x[5] else "plain", STEP.get(by.get(x[3]), "none"))
        if role(x[0], s) == "leader":
            lead[k] += 1
            by_term[(s, (x[0] - T0) // 30_000)] += 1
        else:
            other[k] += 1
total = sum(lead.values())
refused = sum(v for k, v in lead.items() if k[1] == "read-index")
print(f"whole-tier and ls-refs reads on the acting leader: {total}")
print(f"  refused notLeader under the rule (read-index): {refused}")
print(f"  never sent under the rule (plain, after every member declined): {total - refused}")
for k, v in lead.most_common():
    print(f"    {k}: {v}")
print("  on the leader by (site, 30 s bucket):", sorted(by_term.items()))
print(f"plain whole-tier reads on members not leading (the walks' other stops): {sum(v for k, v in other.items() if k[1] == 'plain')}")
