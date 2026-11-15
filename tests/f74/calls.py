#!/usr/bin/env python3
# F74's call trace (decisions/OPEN-QUESTIONS.md): where a member call's time goes, per call, from a run's tsave.mjs
# file (LEARNERS=1 for the learners' rows). Each /raft call carries a per-caller id, so the caller's CallSent (23) and
# AnswerArrived (21) join the callee's CallLeft (24) and CallArrived (20) on (caller address, id):
#   sent    the caller's clock just before the fetch was issued (no I/O between the stamp and the fetch);
#   entered the callee's clock at the top of its fetch handler; left its clock when the handler returned;
#   answer  the caller's clock when the response headers arrived.
# total = answer - sent and work = left - entered are each on one clock. out = entered - sent and back = answer - left
# cross clocks and carry the pair's skew, which cancels in out + back = total - work. The skew per caller and callee
# pair is estimated as (min out - min back) / 2, which assumes the fastest call took the same time each way
# [inference], and out and back are shown corrected by it.
# Workers' clock advances only on I/O: a stamp reads the time of the isolate's last I/O, so a turn spent in JavaScript
# before the stamp is counted in the leg before it and never shows inside a leg [doc, Workers security model]. lag is
# the event-loop lag the ticker saw (host.rs `loop_lag`): the caller's at send, the callee's at entry.
#   python3 tests/f74/calls.py tsave-TAG.jsonl [--from ISO] [--to ISO] [--deadline 1000] [--rows out.tsv]
import collections, datetime, json, sys

args = sys.argv[1:]
path = args[0]
opt = {args[i]: args[i + 1] for i in range(1, len(args) - 1, 2)}
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
T0 = iso(opt["--from"]) if "--from" in opt else 0
T1 = iso(opt["--to"]) if "--to" in opt else 1 << 62
DEADLINE = int(opt.get("--deadline", 1000))

# Every member's rows, by address, deduplicated: tsave asks `since` its last row, so rows repeat only across resets.
rows = collections.defaultdict(set)
for line in open(path):
    r = json.loads(line)
    if "rows" not in r:
        continue
    addr = r["learner"] if "learner" in r else r["site"]
    rows[addr].update(tuple(x) for x in r["rows"])

sent, answer, left, arrived = {}, {}, {}, {}
for addr, rs in rows.items():
    for at, kind, peer, batch, index, ms in rs:
        if not batch:
            continue
        if kind == 23:
            sent[(addr, batch)] = (at, peer, index, ms)
        elif kind == 21:
            answer[(addr, batch)] = at
        elif kind == 24:
            left[(peer, batch)] = (at, addr, index, ms)
        elif kind == 20:
            arrived[(peer, batch)] = (at, ms)

calls = []
for key, (s, to, awaiting, lag) in sent.items():
    if not T0 <= s <= T1:
        continue
    c = {"from": key[0], "to": to, "id": key[1], "sent": s, "awaiting": awaiting, "lag": lag}
    if key in left:
        l, _, entered, clag = left[key]
        c.update(entered=entered, left=l, work=l - entered, clag=clag)
        if key in arrived:
            c["body"] = arrived[key][1]
    if key in answer:
        c["total"] = answer[key] - s
    calls.append(c)

skew = {}
for pair in {(c["from"], c["to"]) for c in calls}:
    outs = [c["entered"] - c["sent"] for c in calls if (c["from"], c["to"]) == pair and "entered" in c]
    backs = [answer[(c["from"], c["id"])] - c["left"] for c in calls
             if (c["from"], c["to"]) == pair and "left" in c and (c["from"], c["id"]) in answer]
    if outs and backs:
        skew[pair] = (min(outs) - min(backs)) // 2
for c in calls:
    k = skew.get((c["from"], c["to"]))
    if k is not None and "entered" in c:
        c["out"] = c["entered"] - c["sent"] - k
        if "total" in c:
            c["back"] = c["total"] - c["work"] - c["out"]


def pct(xs, q):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(q * len(xs)))] if xs else None


def line(name, xs):
    return f"{name} p50/p90/p99/max {pct(xs, .5)}/{pct(xs, .9)}/{pct(xs, .99)}/{max(xs) if xs else None}"


print(f"calls sent {len(calls)}; reached the callee {sum('entered' in c for c in calls)}; "
      f"answered {sum('total' in c for c in calls)}; deadline {DEADLINE} ms")
for pair in sorted({(c["from"], c["to"]) for c in calls}):
    cs = [c for c in calls if (c["from"], c["to"]) == pair]
    get = lambda k: [c[k] for c in cs if k in c]
    print(f"{pair[0]}->{pair[1]} n {len(cs)} skew {skew.get(pair)} | {line('total', get('total'))} | "
          f"{line('out', get('out'))} | {line('work', get('work'))} | {line('back', get('back'))}")

# The calls that missed the deadline, or never got an answer: which leg holds the time.
missed = [c for c in calls if c.get("total", 1 << 62) > DEADLINE]
print(f"missed or unanswered: {len(missed)}")
legs = collections.Counter()
for c in missed:
    if "entered" not in c:
        legs["never reached the callee (in the trace)"] += 1
    elif "total" not in c:
        legs["reached the callee, no answer seen"] += 1
    else:
        part = {"out": c.get("out", 0), "work": c["work"], "back": c.get("back", 0)}
        legs[f"largest leg {max(part, key=part.get)}"] += 1
for k, n in legs.most_common():
    print(f"  {k}: {n}")
# Whether the caller's own state at send explains the out leg: its fetches waiting for headers (a Workers invocation
# queues a seventh until one gets its headers [doc, Workers limits]) and its loop lag.
for name, key, cuts in (("awaiting at send", "awaiting", (1, 6, 12)), ("caller lag at send", "lag", (100, 500, 1000)),
                        ("callee lag at entry", "clag", (100, 500, 1000))):
    print(name)
    lo = 0
    for hi in (*cuts, 1 << 62):
        cs = [c for c in calls if key in c and lo <= c[key] < hi]
        outs = [c["out"] for c in cs if "out" in c]
        late = sum(c.get("total", 1 << 62) > DEADLINE for c in cs)
        print(f"  [{lo}, {hi if hi < 1 << 62 else 'inf'}): n {len(cs)} missed {late} | {line('out', outs)}")
        lo = hi

if "--rows" in opt:
    cols = ["from", "to", "id", "sent", "awaiting", "lag", "entered", "left", "work", "clag", "body", "total", "out",
            "back"]
    with open(opt["--rows"], "w") as f:
        f.write("\t".join(cols) + "\n")
        for c in sorted(calls, key=lambda c: c["sent"]):
            f.write("\t".join(str(c.get(k, "")) for k in cols) + "\n")
