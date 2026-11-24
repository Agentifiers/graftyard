#!/usr/bin/env python3
# B0-02's hosted check for read-round-gap (decisions/DECISIONS.md, "read-round-gap": "at most one read round per voter
# per 20 ms"), from a run's tsave.mjs file. The trace has no read-round kind: a read round is an AppendEntries to every
# voter, so this counts each voter member's `/raft` fetches to each other voter (F74's CallSent, kind 23, one per fetch
# with its `x-graft-sent` stamp) in 20 ms buckets of that stamp. Entry appends and votes are counted too, so the count
# is an upper bound on read rounds, and a bucket over 1 is a candidate breach, not a proven one. Learner addresses
# (>= 128) are callers and callees the rule does not cover. Workers' clock stands still within a turn, so one turn's
# fetches share a stamp: `turns` counts distinct stamps per bucket (fetching turns, at most one read round each),
# which is the closer measure; `max` counts the fetches themselves.
# The read grant: on the leader, a learner's ReadIndex taken (IndexTaken, kind 5) to its ReadIndexRes sent (ResSent,
# kind 6) for the same (peer, batch), on the leader's one clock; p50, p99 and max per serving site.
#   python3 tests/f74/rounds.py tsave-TAG.jsonl [--from ISO] [--to ISO] [--gap 20]
import collections, datetime, json, os, sys

sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'fanout')))
from tracerows import load

args = sys.argv[1:]
path = args[0]
opt = {args[i]: args[i + 1] for i in range(1, len(args) - 1, 2)}
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
T0 = iso(opt["--from"]) if "--from" in opt else 0
T1 = iso(opt["--to"]) if "--to" in opt else 1 << 62
GAP = int(opt.get("--gap", 20))

rows = load(path)

def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(p * len(xs)))] if xs else None

out, grants = {}, {}
for site, rs in sorted(rows.items(), key=lambda kv: str(kv[0])):
    buckets, stamps = collections.Counter(), collections.defaultdict(set)
    for at, kind, peer, batch, index, ms in rs:
        if kind == 23 and peer < 128 and T0 <= at <= T1:
            buckets[(peer, at // GAP)] += 1
            stamps[(peer, at // GAP)].add(at)
    taken, g = {}, []
    for at, kind, peer, batch, index, ms in sorted(rs):
        if kind == 5 and T0 <= at <= T1:
            taken[(peer, batch)] = at
        elif kind == 6 and (peer, batch) in taken:
            g.append(at - taken.pop((peer, batch)))
    if g:
        grants[str(site)] = {"n": len(g), "p50": pct(g, 0.5), "p99": pct(g, 0.99), "max": max(g)}
    per_peer, turns = collections.defaultdict(list), collections.defaultdict(list)
    for (peer, b), n in buckets.items():
        per_peer[peer].append(n)
        turns[peer].append(len(stamps[(peer, b)]))
    out[str(site)] = {
        str(p): {"calls": sum(ns), "buckets": len(ns), "max": max(ns), "over1": sum(1 for n in ns if n > 1),
                 "share_over1": round(sum(1 for n in ns if n > 1) / len(ns), 4),
                 "turns_max": max(turns[p]), "turns_over1": sum(1 for n in turns[p] if n > 1)}
        for p, ns in sorted(per_peer.items())
    }
print(json.dumps({"gapMs": GAP, "from": opt.get("--from"), "to": opt.get("--to"), "bySite": out, "grantMs": grants}, indent=1))
