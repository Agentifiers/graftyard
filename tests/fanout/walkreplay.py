#!/usr/bin/env python3
"""walkreplay.py TSAVE.jsonl[.xz] FROM_ISO TO_ISO -> what `lookup-retry` would remove from the followers' inbound
load in a B0-33 run, replayed on its member trace (test-faults: `Trace::Entered` 25 and `Trace::SentBy` 27).

Walks. A follower's `/read` sent by a walk (`SentBy::Walk`) comes from a lookup pass whose post to the leader missed
(no answer in CALL, a 5xx, or `busy`). Under main's rule that pass posts every follower, ends, forgets the leader,
waits 250 ms, and posts the leader again first. Under `lookup-retry` the first miss posts no follower: the request
reports, waits LEADER_RETRY, and posts the leader again, and only a second miss walks. So a walk survives only when the
retry misses too, which under main's rule shows as the same isolate walking the same follower again one leader post
later. Each walk is matched to that isolate's next walk at the follower: a gap within [REPEAT_LO, REPEAT_HI] ms (a
pass's 250 ms plus one post to the leader that ran to CALL, 3.5 s) is counted as a second miss. That is an upper bound
on the walks that survive, because a later walk inside that gap can belong to a different request.

Reports. A follower's `/suspect` is damped per isolate per SUSPECT_GAP. The replay keys each report by its sending
isolate only (nearly every report in a B0-33 run is about one leader in one term), so it gives the lower bound on what
survives at 5 s; the cap at that gap is the isolates over 5 s.
  python3 tests/fanout/walkreplay.py tests/fanout/raw/tsave-m45b33.jsonl.xz 2026-11-17T22:24:20Z 2026-11-17T22:29:20Z"""
import collections, datetime, json, lzma, sys

ROUTES = ["/raft", "/read", "/commit", "/commit_many", "/outcome", "/outcome_by_request", "/suspect"]
WALK, REPEAT_LO, REPEAT_HI = 2, 3_000, 4_500
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
path, A, B = sys.argv[1], iso(sys.argv[2]), iso(sys.argv[3])
secs = (B - A) / 1000
rows = collections.defaultdict(set)
GAPS = (500, 1_000, 2_000, 2_500, 5_000)

for line in (lzma.open if path.endswith(".xz") else open)(path, "rt"):
    r = json.loads(line)
    if "rows" in r and "learner" not in r:
        rows[r["site"]].update(tuple(x) for x in r["rows"] if x[1] in (25, 27))
leader = None
for s in sorted(rows):
    entered = {x[4] // 64: (x[0], x[4] % 64) for x in rows[s] if x[1] == 25}
    sent = {x[3]: (x[4], x[5]) for x in rows[s] if x[1] == 27}
    walks = collections.defaultdict(list)
    reports = collections.defaultdict(list)
    total = collections.Counter()
    for arrival, (at, route) in entered.items():
        if not A <= at < B:
            continue
        name = ROUTES[route - 1] if 0 < route <= len(ROUTES) else str(route)
        total[name] += 1
        isolate, step = sent.get(arrival, (None, None))
        if name == "/read" and step == WALK:
            walks[isolate].append(at)
        if name == "/suspect":
            reports[isolate].append(at)
    nwalk = sum(map(len, walks.values()))
    repeat = 0
    for ts in walks.values():
        ts.sort()
        repeat += sum(1 for a, b in zip(ts, ts[1:]) if REPEAT_LO <= b - a <= REPEAT_HI)
    kept = {}
    for gap in GAPS:
        n = 0
        for ts in reports.values():
            last = None
            for t in sorted(ts):
                if last is None or t >= last + gap:
                    n, last = n + 1, t
        kept[gap] = n
    nrep = sum(map(len, reports.values()))
    print(f"voter {s}: entered {sum(total.values()) / secs:.0f}/s (/read {total['/read'] / secs:.0f}, "
          f"/suspect {total['/suspect'] / secs:.0f}, /raft {total['/raft'] / secs:.0f}); "
          f"walk /reads {nwalk / secs:.1f}/s from {len(walks)} isolates, of which a second walk one leader post later "
          f"{repeat / secs:.1f}/s; /suspect {nrep / secs:.1f}/s from {len(reports)} isolates, kept per isolate at "
          + ", ".join(f"{g} ms {kept[g] / secs:.1f}/s" for g in GAPS) + f" (cap at 5 s {len(reports) / 5:.0f}/s)")
