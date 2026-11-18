#!/usr/bin/env python3
"""txmix.py ROW TSAVE FROM TO TERMS [SHIFT_MS] -> what a leader's deferred transaction writes, per confirm.

ROW is the a1 row file (`raw/a1-b0-33-mNN.jsonl`): its leader `/stats` `bigTx` holds the statement mix (verb and table,
count, argument bytes) of the leader's newest 20 transactions of 100 KB or more (`host::stmt_mix`). TSAVE gives every
deferred transaction's statement count (`Flushed` 16, `index`) and each member's `Confirm` (10) and `Applied` (4) rows.

Prints:
1. the leader's deferred transactions by size: count, statements p50/p99, and their total;
2. the `bigTx` mix grouped by what writes it, for transactions carrying an `ExtendLeases` (100 or more `INSERT c_lease`)
   and for the rest, per transaction;
3. an estimate of each group's share of all the leader's deferred statements: the size classes' totals from (1) times
   the matching mix from (2) (the small class's mix is sampled from its transactions of 100 KB or more);
4. the leader's sync after each deferred transaction against the followers' first sync after they applied the same
   index. A follower applies the same entries and writes the same fold rows (`SqlStore`, `Write::Apply`), but in an
   immediate transaction that `Flushed` does not trace, so its statements are not counted here.
  python3 tests/fanout/txmix.py tests/fanout/raw/a1-b0-33-m52.jsonl tests/fanout/raw/tsave-m52b33.jsonl.xz \\
    2026-11-18T14:44:21Z 2026-11-18T14:49:21Z tests/fanout/raw/terms-m52b33.jsonl
SHIFT_MS moves TERMS onto the trace's clock for runs before srvclock (m50 +7310, m51 +7440)."""
import bisect, collections, datetime, json, lzma, sys

ROW, P, F, T, TERMS = sys.argv[1:6]
SHIFT = int(sys.argv[6]) if len(sys.argv) > 6 else 0
BIG = 1000
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
T0, T1 = iso(F), iso(T)
pc = lambda v, p: sorted(v)[min(len(v) - 1, int(p * len(v)))] if v else None


def group(kind):
    verb, t = kind.split(" ", 1)
    if t in ("log", "raft_state"):
        return "raft log and state"
    if t == "request":
        return "request rows" if verb == "INSERT" else "request retention DELETE"
    if t == "dup":
        return "dup (test-faults)"
    if t.startswith("c_lease"):
        return "lease rows"
    if "agent_task" in t:
        return "agent task rows"
    if "comment" in t:
        return "comment rows (with fts, by_issue)"
    if "issue" in t or "pull" in t or "open_head" in t:
        return "issue and PR rows (with fts)"
    if t.endswith(("__fold", "__digest", "_ids")) or t == "c_counter":
        return "tier bookkeeping (_fold, _digest, ids)"
    if t == "c_ref":
        return "ref rows"
    if t == "m_path_set":
        return "path_set rows"
    return "other"


row = next(d for d in map(json.loads, open(ROW)) if isinstance(d.get("leader"), dict))
big_tx = row["leader"].get("bigTx", [])

rows = collections.defaultdict(set)
for line in (lzma.open if P.endswith(".xz") else open)(P, "rt"):
    r = json.loads(line)
    if "rows" in r and "learner" not in r:
        rows[r["site"]].update(tuple(x) for x in r["rows"] if x[1] in (4, 10, 16))
X = {s: sorted(v) for s, v in rows.items()}
samples = [(iso(r["at"]) + SHIFT, {m[0]: m[1] for m in r["members"]}) for r in map(json.loads, open(TERMS)) if r.get("members")]
st = [t for t, _ in samples]


def role(t, s):
    return samples[max(0, bisect.bisect_right(st, t) - 1)][1].get(s, "?")


print("1. leader deferred transactions (Flushed), by statements")
totals = {"big": 0, "small": 0}
for s in sorted(X):
    fl = [x[4] for x in X[s] if x[1] == 16 and T0 <= x[0] < T1 and role(x[0], s) == "leader"]
    if not fl:
        continue
    big = [n for n in fl if n >= BIG]
    totals["big"] += sum(big)
    totals["small"] += sum(fl) - sum(big)
    print(f"  member {s}: {len(fl)} transactions, statements p50 {pc(fl, .5)} p99 {pc(fl, .99)} max {max(fl)}, "
          f"total {sum(fl)}; {len(big)} of {BIG}+ hold {sum(big)} ({100 * sum(big) / sum(fl):.0f}%)")

print(f"\n2. bigTx mix (the leader's newest {len(big_tx)} transactions of 100 KB or more), per transaction")
is_ext = lambda b: dict((k, n) for k, n, _ in b["mix"]).get("INSERT c_lease", 0) >= 100
mixes = {}
for name, g in (("big", [b for b in big_tx if is_ext(b)]), ("small", [b for b in big_tx if not is_ext(b) and b["stmts"] > 2])):
    if not g:
        continue
    c = collections.Counter()
    for b in g:
        for k, n, _ in b["mix"]:
            c[group(k)] += n
    tot = sum(b["stmts"] for b in g)
    mixes[name] = {k: n / tot for k, n in c.items()}
    label = "with ExtendLeases" if name == "big" else "without"
    print(f"  {label}: {len(g)} transactions, {min(b['stmts'] for b in g)} to {max(b['stmts'] for b in g)} statements")
    for k, n in c.most_common():
        print(f"    {k:40} {n / len(g):7.1f} per transaction  {100 * n / tot:5.1f}%")

print("\n3. estimated share of all the leader's deferred statements")
est = collections.Counter()
for cls, tot in totals.items():
    for k, f in mixes.get(cls, {}).items():
        est[k] += f * tot
whole = sum(totals.values())
for k, n in est.most_common():
    print(f"  {k:40} {n:8.0f}  {100 * n / whole:5.1f}%")

print("\n4. leader sync after a deferred transaction, against the followers' sync after applying the same index")
res = {"big": ([], []), "small": ([], [])}
for L in X:
    xs = [x for x in X[L] if T0 <= x[0] < T1 and role(x[0], L) == "leader"]
    ap = [x for x in xs if x[1] == 4]
    apt = [x[0] for x in ap]
    for j, x in enumerate(xs):
        if x[1] != 16:
            continue
        k = bisect.bisect_right(apt, x[0]) - 1
        lc = next((y for y in xs[j:] if y[1] == 10), None)
        if k < 0 or not lc:
            continue
        key = "big" if x[4] >= BIG else "small"
        res[key][0].append(lc[5])
        for f in X:
            if f == L:
                continue
            fs = [y for y in X[f] if x[0] - 2000 <= y[0] < x[0] + 30000]
            a = next((y for y in fs if y[1] == 4 and y[4] >= ap[k][4]), None)
            if not a or role(a[0], f) != "follower":
                continue
            c = next((y for y in fs if y[1] == 10 and y[0] >= a[0]), None)
            if c:
                res[key][1].append(c[5])
for key, (lead, fol) in res.items():
    if lead:
        print(f"  {key:5} ({BIG}+ statements: {key == 'big'}): leader n {len(lead)} p50 {pc(lead, .5)} p90 {pc(lead, .9)} "
              f"max {max(lead)} at 1 s or more {sum(v >= 1000 for v in lead)}; followers n {len(fol)} p50 {pc(fol, .5)} "
              f"p90 {pc(fol, .9)} max {max(fol, default=None)} at 1 s or more {sum(v >= 1000 for v in fol)}")
