#!/usr/bin/env python3
# F74's inbound trace (decisions/OPEN-QUESTIONS.md, F74): what entered a callee's handler while a member's /raft call
# to it was still on its way. Reads a run's tsave.mjs file from a test-faults build with `Trace::Entered` (kind 25):
# one row per request at the top of each member's handler, [at, 25, from, call id, arrival * 64 + route, open].
#   from     the sender's address (x-graft-from; 255 for an edge or harness request)
#   call id  the /raft call's x-graft-call (0 for any other route), which joins the caller's CallSent (23)
#   arrival  the request's number on that member's host, from 1; it restarts when the object resets
#   route    the code in host.rs `ROUTES`, kept the same below (append-only)
#   open     requests in that host's handler when this one entered, itself included
# For each voter-to-voter /raft call it finds the callee's own Entered row by (caller, call id), takes the out leg as
# calls.py does (entered - sent, corrected by the pair's skew estimate (min out - min back) / 2 [inference]), and
# counts the requests that entered the callee during that leg: arrivals after the callee clock reads entered - out,
# and before the call's own arrival number. Then it groups calls by out leg.
# Workers' clock advances only on I/O, so stamps from one isolate turn share a value [doc, Workers security model];
# counting "entered ahead" by arrival number, not by clock, is exact on the callee side.
# Completeness: within one host segment the arrival numbers run 1, 2, 3, ...; a gap is rows lost from the trace ring
# (TRACE_MAX) or between tsave reads, and the check reports them, with any row whose clock runs backwards.
#   python3 tests/f74/inbound.py tsave-TAG.jsonl [--from ISO] [--to ISO] [--deadline 1000] [--rows out.tsv]
import bisect, collections, datetime, json, sys

ROUTES = ["/raft", "/read", "/commit", "/commit_many", "/outcome", "/outcome_by_request", "/suspect", "/campaign",
          "/status", "/stats", "/trace", "/ping", "/rtt", "/place", "/learners", "/record"]
route = lambda code: ROUTES[code - 1] if 0 < code <= len(ROUTES) else "other"

args = sys.argv[1:]
path = args[0]
opt = {args[i]: args[i + 1] for i in range(1, len(args) - 1, 2)}
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
T0 = iso(opt["--from"]) if "--from" in opt else 0
T1 = iso(opt["--to"]) if "--to" in opt else 1 << 62
DEADLINE = int(opt.get("--deadline", 1000))
VOTER = lambda a: isinstance(a, int) and a < 128

rows = collections.defaultdict(set)
for line in open(path):
    r = json.loads(line)
    if "rows" in r:
        rows[r["learner"] if "learner" in r else r["site"]].update(tuple(x) for x in r["rows"])


def pct(xs, q):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(q * len(xs)))] if xs else None


# Each member's Entered rows, split into host segments where the arrival number restarts.
segments = {}
print("completeness (per member: rows, segments, arrival numbers missing inside segments, clock going backwards)")
for addr in sorted(rows, key=str):
    ent = [r for r in rows[addr] if r[1] == 25]
    if not ent:
        continue
    # A new host segment starts at each arrival number 1 (the object reset, and its count restarted).
    segs = []
    for r in sorted(ent, key=lambda r: (r[0], r[4] // 64)):
        if r[4] // 64 == 1 or not segs:
            segs.append([])
        segs[-1].append(r)
    missing = back = 0
    for s in segs:
        s.sort(key=lambda r: r[4] // 64)
        nums = [r[4] // 64 for r in s]
        missing += (nums[-1] - nums[0] + 1) - len(set(nums))
        back += sum(1 for a, b in zip(s, s[1:]) if b[0] < a[0])
    segments[addr] = segs
    print(f"  {addr}: rows {len(ent)} segments {len(segs)} missing {missing} backwards {back}")

# Per callee: (arrival number -> row) and the clock to arrival-number index, per segment.
by_call, index = {}, {}
for addr, segs in segments.items():
    for k, s in enumerate(segs):
        at = [r[0] for r in s]
        index[(addr, k)] = (at, s)
        for r in s:
            if r[2] != 255 and r[3]:
                by_call[(r[2], r[3])] = (addr, k, r)

sent, answer = {}, {}
for addr, rs in rows.items():
    for at, kind, peer, batch, idx, ms in rs:
        if kind == 23 and batch:
            sent[(addr, batch)] = (at, peer, idx)
        elif kind == 21 and batch:
            answer[(addr, batch)] = at
left = {(peer, batch): at for addr, rs in rows.items() for at, kind, peer, batch, idx, ms in rs if kind == 24 and batch}

calls = []
for key, (s, to, awaiting) in sent.items():
    if not (T0 <= s <= T1 and VOTER(key[0]) and VOTER(to)) or key not in by_call:
        continue
    addr, seg, e = by_call[key]
    c = {"from": key[0], "to": to, "id": key[1], "sent": s, "entered": e[0], "arrival": e[4] // 64, "open": e[5],
         "seg": seg, "awaiting": awaiting}
    if key in answer:
        c["total"] = answer[key] - s
        if key in left:
            c["back_raw"] = answer[key] - left[key]
    calls.append(c)

skew = {}
for pair in {(c["from"], c["to"]) for c in calls}:
    outs = [c["entered"] - c["sent"] for c in calls if (c["from"], c["to"]) == pair]
    backs = [c["back_raw"] for c in calls if (c["from"], c["to"]) == pair and "back_raw" in c]
    if outs and backs:
        skew[pair] = (min(outs) - min(backs)) // 2

for c in calls:
    k = skew.get((c["from"], c["to"]), 0)
    c["out"] = max(0, c["entered"] - c["sent"] - k)
    at, seg = index[(c["to"], c["seg"])]
    lo = bisect.bisect_left(at, c["entered"] - c["out"])
    ahead = [r for r in seg[lo:] if r[4] // 64 < c["arrival"]]
    c["ahead"] = len(ahead)
    c["mix"] = collections.Counter(route(r[4] % 64) for r in ahead)
    c["ahead_edge"] = sum(1 for r in ahead if r[2] == 255)

print(f"\nvoter-to-voter /raft calls joined to the callee's Entered row: {len(calls)}"
      f" (pair skew estimates: {dict(sorted(skew.items()))})")
names = ["/raft", "/read", "/commit", "/suspect", "/outcome_by_request", "/outcome", "other"]
print("out leg (ms)   n      ahead p50/p90   edge-sent p50   open at entry p50/p90   ahead by route, p50 share")
for lo, hi in [(0, 200), (200, 1000), (1000, 3000), (3000, 1 << 62)]:
    g = [c for c in calls if lo <= c["out"] < hi]
    if not g:
        continue
    share = {n: pct([c["mix"].get(n, 0) / c["ahead"] for c in g if c["ahead"]], 0.5) for n in names}
    mix = " ".join(f"{n} {v:.2f}" for n, v in share.items() if v)
    print(f"[{lo}, {hi if hi < 1 << 62 else 'inf'})".ljust(15) + f"{len(g):<7}{pct([c['ahead'] for c in g], .5)}/"
          f"{pct([c['ahead'] for c in g], .9)}".ljust(16) + f"{pct([c['ahead_edge'] for c in g], .5)}".ljust(16)
          + f"{pct([c['open'] for c in g], .5)}/{pct([c['open'] for c in g], .9)}".ljust(24) + mix)
missed = [c for c in calls if c.get("total", DEADLINE) >= DEADLINE]
print(f"missed {DEADLINE} ms or unanswered: {len(missed)}; requests entered ahead p50 {pct([c['ahead'] for c in missed], .5)}")

if "--rows" in opt:
    with open(opt["--rows"], "w") as f:
        cols = ["from", "to", "id", "sent", "entered", "out", "total", "arrival", "open", "ahead", "ahead_edge"] + names
        f.write("\t".join(cols) + "\n")
        for c in sorted(calls, key=lambda c: c["sent"]):
            f.write("\t".join(str(c.get(k, c["mix"].get(k, ""))) for k in cols) + "\n")
