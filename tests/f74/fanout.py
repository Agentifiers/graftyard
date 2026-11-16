#!/usr/bin/env python3
# F74's fan-out reading (tests/m0/results/fanout-dups.md): where a voter-to-voter /raft call's legs go, by its place
# in the caller's turn. A turn is every CallSent (23) one voter stamped at one clock reading; Workers' clock moves only
# on I/O, so one turn's sends share a stamp [doc, Workers security model]. legs = total - work, each on one clock, so
# the pair's skew cancels. A Workers invocation lets six fetches wait for response headers at once and queues the rest
# [doc, https://developers.cloudflare.com/workers/platform/limits/#simultaneous-open-connections], so a call past a
# turn's sixth should not be answered, less the callee's work, before one of the first six was. It prints:
#   - legs by place in the turn (turns of six or fewer, calls 1-6, 7-12, 13+ of a larger turn);
#   - calls past the sixth answered (less callee work) more than 300 ms before any of the turn's first six;
#   - calls overtaken by a later-sent call from the same caller to the same callee, delayed (out >= 1 s) or prompt;
#   - the caller's fetches awaiting at send, delayed and prompt;
#   - forced fan-out: turns with 10 or more calls to one voter, and how many of their AppendQueued (12) rows repeat a
#     last index already queued to that voter in the same turn.
#   python3 tests/f74/fanout.py tsave-TAG.jsonl [--from ISO] [--to ISO]
import collections, datetime, json, sys

args = sys.argv[1:]
opt = {args[i]: args[i + 1] for i in range(1, len(args) - 1, 2)}
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
T0 = iso(opt["--from"]) if "--from" in opt else 0
T1 = iso(opt["--to"]) if "--to" in opt else 1 << 62
V = (0, 1, 2)

rows = collections.defaultdict(set)
for line in open(args[0]):
    r = json.loads(line)
    if "rows" in r:
        rows[r["learner"] if "learner" in r else r["site"]].update(tuple(x) for x in r["rows"])


def pct(xs, q):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(q * len(xs)))] if xs else None


sent, entered, answer, left = {}, {}, {}, {}
for addr, rs in rows.items():
    for at, kind, peer, batch, index, ms in rs:
        if not batch:
            continue
        if kind == 23:
            sent[(addr, batch)] = (at, peer, index)
        elif kind == 25:
            entered[(peer, batch)] = at
        elif kind == 21:
            answer[(addr, batch)] = at
        elif kind == 24:
            left[(peer, batch)] = (at, index)


def legs(k):
    if k in answer and k in left:
        return (answer[k] - sent[k][0]) - (left[k][0] - left[k][1])


turns = collections.defaultdict(list)
for k, (at, to, _) in sent.items():
    if k[0] in V and to in V and T0 <= at <= T1:
        turns[(k[0], at)].append(k)
place, early = collections.defaultdict(list), [0, 0]
for (caller, at), ks in turns.items():
    ks.sort(key=lambda k: k[1])
    for i, k in enumerate(ks):
        if legs(k) is not None:
            place["<=6" if len(ks) <= 6 else ("1-6", "7-12", "13+")[min(i // 6, 2)]].append(legs(k))
    first = [answer[k] for k in ks[:6] if k in answer]
    if len(ks) > 6 and len(first) == 6:
        for k in ks[6:]:
            if legs(k) is not None:
                early[1] += 1
                early[0] += answer[k] - (left[k][0] - left[k][1]) < min(first) - 300
print("voter-to-voter legs (total - work) by place in the caller's turn")
for p in ("<=6", "1-6", "7-12", "13+"):
    print(f"  {p:5} n {len(place[p]):6} p50 {pct(place[p], .5)} p90 {pct(place[p], .9)}")
print(f"calls past a turn's sixth answered (less work) >300 ms before any of its first six: {early[0]} of {early[1]}")

pairs = collections.defaultdict(list)
for k, (at, to, aw) in sent.items():
    if k[0] in V and to in V and k in entered and T0 <= at <= T1:
        pairs[(k[0], to)].append((at, k[1], entered[k], aw))
over, awaiting = collections.Counter(), collections.defaultdict(list)
for cs in pairs.values():
    cs.sort()
    for i, (at, cid, ent, aw) in enumerate(cs):
        tag = "delayed" if ent - at >= 1000 else "prompt"
        over[(tag, "calls")] += 1
        over[(tag, "overtaken")] += any(d[0] > at and d[2] < ent for d in cs[i + 1:i + 400])
        awaiting[tag].append(aw)
for tag in ("delayed", "prompt"):
    print(f"{tag} (out >= 1 s on raw clocks: {tag == 'delayed'}): {over[(tag, 'calls')]} calls, "
          f"{over[(tag, 'overtaken')]} overtaken by a later-sent call to the same callee; "
          f"caller awaiting at send p50 {pct(awaiting[tag], .5)} p90 {pct(awaiting[tag], .9)}")

print("forced fan-out: turns with >= 10 calls to one voter")
for addr in V:
    rs = [x for x in rows[addr] if T0 <= x[0] <= T1]
    n = collections.Counter((at, p) for at, kind, p, b, i, ms in rs if kind == 23 and p in V)
    big = {k for k, c in n.items() if c >= 10}
    queued = collections.defaultdict(list)
    for at, kind, p, b, i, ms in rs:
        if kind == 12 and (at, p) in big:
            queued[(at, p)].append(i)
    q = sum(len(v) for v in queued.values())
    dup = sum(len(v) - len(set(v)) for v in queued.values())
    print(f"  site {addr}: turns {len(big)}, calls {sum(n[k] for k in big)} of {sum(n.values())}; "
          f"AppendQueued {q}, repeating a last index already queued that turn {dup}")
