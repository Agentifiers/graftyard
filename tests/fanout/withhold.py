#!/usr/bin/env python3
"""withhold.py TSAVE FROM TO TERMS -> when the voters' SUSPECT rule could withhold promises (`raft::SUSPECT`: reports
from 2 distinct edge colos within 10 s, about the promised leader, while the follower's last progressing append is
CALL_DEADLINE or more old). Progress is approximated by the follower's last `AppendStepped` (17) row that carried a new
entry or commit; reports are `Reported` (28), colo as ASCII in a u32, 0 a site script (never counted). Prints, per run,
how many follower-seconds met the rule, with one colo or two, and the leader changes that followed within 20 s."""
import bisect, collections, datetime, json, lzma, sys

P, F, T, TERMS = sys.argv[1:5]
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
T0, T1 = iso(F), iso(T)
rep, app = collections.defaultdict(list), collections.defaultdict(list)
for line in (lzma.open if P.endswith(".xz") else open)(P, "rt"):
    d = json.loads(line)
    if "rows" not in d or d.get("learner") is not None:
        continue
    for x in d["rows"]:
        if x[1] == 28:
            rep[d["site"]].append((x[0], x[2], x[4]))
        elif x[1] == 17:
            app[d["site"]].append((x[0], x[3], x[5]))
S = [(iso(d["at"]), {m[0]: (m[1], m[2]) for m in d["members"]}) for d in map(json.loads, open(TERMS)) if d.get("members")]
st = [t for t, _ in S]
at = lambda t: S[max(0, bisect.bisect_right(st, t) - 1)][1]
leader_at = lambda t: next((s for s, (r, _) in at(t).items() if r == "leader"), None)
changes, last = [], None
for t, m in S:
    l = next(((s, tm) for s, (r, tm) in m.items() if r == "leader"), None)
    if l and l != last:
        changes.append((t, l))
        last = l
fire = {1: set(), 2: set()}
for f in (0, 1, 2):
    A = sorted(set(app[f]))
    prog, best = [], (0, 0)
    for t, idx, commit in A:  # an append that moved the last index or the commit
        if (idx, commit) > best:
            prog.append(t)
            best = max(best, (idx, commit))
    R = sorted(set(r for r in rep[f] if T0 <= r[0] < T1))
    for i, (t, about, colo) in enumerate(R):
        if colo == 0 or leader_at(t) != about or leader_at(t) == f:
            continue
        k = bisect.bisect_right(prog, t) - 1
        if k >= 0 and t - prog[k] < 1000:
            continue
        colos = {c for (u, a, c) in R[max(0, i - 400):i + 1] if t - 10000 <= u <= t and c and a == about}
        fire[min(2, len(colos))].add((f, t // 1000))
first = lambda secs: sorted({s for _, s in secs})
for n in (1, 2):
    print(f"follower-seconds where a late leader drew reports from {n} colo(s){'+' if n == 2 else ''}: {len(fire[n])}")
w = first(fire[2])
print("leader changes in the window:", sum(T0 <= t < T1 for t, _ in changes) )
hit = sum(1 for t, _ in changes if T0 <= t < T1 and any(t / 1000 - 20 <= s <= t / 1000 for s in w))
print("  preceded within 20 s by a 2-colo late second (the rule withholds):", hit)
