#!/usr/bin/env python3
"""withhold.py TSAVE.jsonl[.xz] TERMS.jsonl -> why a B0-33 group's followers stopped renewing their promises.

For every (follower, term) whose promise lapsed, it bounds the follower's last renewal from terms.mjs's samples
(`promiseUntil` - member `now`, read inside [at, at + ms], ELECTION_AFTER = 10 s), then from the member trace
(tsave.mjs, test-faults):
  - appends from the leader after it: how long until a newer entry came, and whether one of the same term came more
    than LEASE (9 s) after the last renewal, which a transfer could not explain (a transfer ends within LEASE);
  - `/suspect` reports joined to their sender (`Trace::Entered` route 7, `SentBy` 27): an edge commit isolate (it
    sent `/commit` or `/outcome_by_request`), a site-script isolate (it sent with step 0 on a route other than a known
    edge one), or a read-only isolate;
  - a replay of election.rs's SUSPECT rule (2 distinct colos in 10 s withhold for 20 s, a higher term clears it) on
    the follower's own appends and reports from its first append of the term, with every report from colo LAX, then with site-script and read-only
    isolates at colo 0 (`Colo::UNKNOWN`, `colo-unknown`), and the predicted last renewal against the bound.
Members log no report's colo, `about` or term before `Trace::Reported` (28), so the replay assumes the colo from the
sender's class and counts every report, also stale ones the member would skip: an upper bound on withholding.
  python3 tests/fanout/withhold.py tests/fanout/raw/tsave-m43b33.jsonl.xz tests/fanout/raw/terms-m43b33.jsonl"""
import collections, datetime, json, lzma, sys

ELECTION_AFTER, LEASE, WINDOW, SLACK = 10_000, 9_000, 10_000, 100
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
hm = lambda t: datetime.datetime.fromtimestamp(t / 1000, datetime.UTC).strftime("%M:%S.%f")[:-3] if t else "-"

tpath, spath = sys.argv[1], sys.argv[2]
rows = collections.defaultdict(set)
for line in (lzma.open if tpath.endswith(".xz") else open)(tpath, "rt"):
    r = json.loads(line)
    if "rows" in r and "learner" not in r:
        rows[r["site"]].update(tuple(x) for x in r["rows"])

# Sender classes: each isolate's (route, step) pairs across the voters.
routes = collections.defaultdict(collections.Counter)
for s in rows:
    entered = {x[4] // 64: x[4] % 64 for x in rows[s] if x[1] == 25}
    for x in rows[s]:
        if x[1] == 27:
            routes[x[4]][(entered.get(x[3]), x[5])] += 1


def kind(i):
    c = routes.get(i, {})
    if any(step in (4, 5) for _, step in c):
        return "edge"
    if any(route in (None, 0) for route, _ in c):
        return "site"
    return "read"


reports = {}
for s in rows:
    sender = {x[3]: x[4] for x in rows[s] if x[1] == 27}
    reports[s] = sorted((x[0], kind(sender.get(x[4] // 64))) for x in rows[s] if x[1] == 25 and x[4] % 64 == 7)
print("isolates by class:", dict(collections.Counter(kind(i) for i in routes)))
print("/suspect by sender class:", dict(sum((collections.Counter(k for _, k in reports[s]) for s in reports), collections.Counter())))

samples = [json.loads(l) for l in open(spath) if '"members"' in l]
start, leader = {}, {}
for sm in samples:
    for m in sm["members"]:
        if m[1] == "leader":
            start.setdefault(m[2], iso(sm["at"]))
            leader.setdefault(m[2], m[0])
promises = collections.defaultdict(list)
for sm in samples:
    a = iso(sm["at"])
    for m in sm["members"]:
        if m[1] == "follower" and m[5] is not None and m[2] is not None:
            promises[(m[0], m[2])].append((a + m[5], a + sm["ms"] + m[5]))


def last_lapse(bounds):
    lo, hi = -1, 1 << 62
    for a, b in bounds:
        lo, hi = (max(lo, a), min(hi, b)) if max(lo, a) <= min(hi, b) + SLACK else (a, b)
    return lo, hi


def replay(fo, term, colo):
    """The last append from `term`'s leader that the SUSPECT rule lets renew, with reports' colos from `colo`."""
    t0 = start[term] - 3_000
    t1 = min([t for k, t in start.items() if k > term], default=1 << 62) + 15_000
    ev = [(x[0], 0, None) for x in rows[fo] if x[1] == 17 and x[2] == leader[term] and t0 <= x[0] < t1]
    if not ev:
        return None
    # A report counts only while the follower's promise to `about` runs, so from its first append from this leader.
    first = min(t for t, _, _ in ev)
    ev += [(t, 1, colo(k)) for t, k in reports[fo] if first <= t < t1]
    seen, until, last = {}, 0, None
    for t, typ, c in sorted(ev, key=lambda e: (e[0], e[1])):
        if typ:
            seen = {k: v for k, v in seen.items() if v >= t - WINDOW}
            seen[c] = t
            if len(seen) >= 2:
                until = max(until, t + 2 * WINDOW)
        elif t >= until:
            last = t
    return last


one = lambda k: "LAX"
unknown = lambda k: "LAX" if k == "edge" else 0
n = late = hit_one = hit_unknown = 0
for (fo, term), bounds in sorted(promises.items(), key=lambda k: (k[0][1], k[0][0])):
    if term not in leader or fo == leader[term]:
        continue
    lo, hi = last_lapse(bounds)
    nxt = min([t for k, t in start.items() if k > term], default=None)
    if hi - lo > 4_000 or hi - ELECTION_AFTER < start[term] or nxt is None:
        continue  # no lapse pinned within 4 s, or none inside this term
    s0, s1 = lo - ELECTION_AFTER, hi - ELECTION_AFTER
    appends = sorted((x[0], x[4]) for x in rows[fo] if x[1] == 17 and x[2] == leader[term])
    top = max([i for t, i in appends if t <= s1], default=0)
    newer = next((t for t, i in appends if t > s1 and i > top), None)
    past = any(s0 + LEASE + SLACK < t < hi for t, _ in appends)
    p1, p0 = replay(fo, term, one), replay(fo, term, unknown)
    inside = lambda p: p is not None and s0 - 400 <= p <= s1 + 400
    n, late = n + 1, late + past
    hit_one, hit_unknown = hit_one + inside(p1), hit_unknown + inside(p0)
    kinds = collections.Counter(k for t, k in reports[fo] if s0 - WINDOW <= t <= s1)
    print(f"t{term:2d} f{fo}: last renewal [{hm(s0)},{hm(s1)}], next term at {hm(nxt)}; newer entry after "
          f"{'-' if newer is None else round((newer - s1) / 1000, 1)} s; append past LEASE {'yes' if past else 'no'}; "
          f"reports in the 10 s before {dict(kinds)}; replay one colo {hm(p1)}, colo 0 {hm(p0)} {'IN' if inside(p0) else ''}")
print(f"lapses {n}: an append of the term more than LEASE after the last renewal in {late}; "
      f"replayed last renewal inside the bound: one colo {hit_one}, site-script and read-only isolates at colo 0 {hit_unknown}")
