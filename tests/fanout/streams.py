#!/usr/bin/env python3
"""streams.py TSAVE FROM TO TERMS -> the repository leader's inbound by stream, per member while it led, from a tsave
file with Entered (25), ReadQuery (26) and Lag (8) rows. Streams keep the causes apart (orchestrator, B0-33 m52):
`/raft` from learners (sender >= 192) and from voters, the commit path (`/commit`, `/commit_many`, `/outcome`,
`/outcome_by_request`), `/read` split by query (`refs` is v2 ls-refs's refs pages, F91; `packed`, `core`, `rows`, ...),
`/suspect`, and the rest. Beside each: requests open at entry (p50, p90), and the ticker-stall share of the
leader's seconds by stream rate band. Times on Cloudflare's clock; TERMS from terms.mjs (srvclock-stamped since
254fd6d5).
  python3 tests/fanout/streams.py tests/fanout/raw/tsave-m52b33.jsonl 2026-11-18T14:39:20Z 2026-11-18T14:44:20Z \\
    tests/fanout/raw/terms-m52b33.jsonl"""
import bisect, collections, datetime, json, lzma, sys

P, F, T, TERMS = sys.argv[1:5]
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
T0, T1 = iso(F), iso(T)
R = ["/raft", "/read", "/commit", "/commit_many", "/outcome", "/outcome_by_request", "/suspect", "/campaign", "/status",
     "/stats", "/trace", "/ping", "/rtt", "/place", "/learners", "/record"]  # host.rs ROUTES, code i + 1; 0 any other
Q = ["probe", "dep", "rows", "prsync", "accountLogins", "user", "summary", "split", "settings", "request", "repoOwner",
     "repo", "ranges", "placement", "mergequeue", "login", "jti", "hosted", "genesis", "app", "key", "packed", "refs",
     "core"]  # workers/site/src/host.rs READ_QUERIES, code i + 1
pct = lambda xs, q: (sorted(xs)[min(len(xs) - 1, int(q * len(xs)))] if xs else None)

rows = collections.defaultdict(set)
for line in (lzma.open if P.endswith(".xz") else open)(P, "rt"):
    r = json.loads(line)
    if "rows" in r and "learner" not in r:
        rows[r["site"]].update(tuple(x) for x in r["rows"] if x[1] in (8, 25, 26))
samples = [(iso(r["at"]), {m[0]: m[1] for m in r["members"]}) for r in map(json.loads, open(TERMS)) if r.get("members")]
st = [t for t, _ in samples]


def leader(t):
    i = max(0, bisect.bisect_right(st, t) - 1)
    return next((k for k, v in samples[i][1].items() if v == "leader"), None)


def stream(x, q):
    r = x[4] % 64
    name = R[r - 1] if 0 < r <= len(R) else "unlisted"
    if name == "/raft":
        return "raft:learner" if x[2] >= 192 else "raft:voter"
    if name in ("/commit", "/commit_many", "/outcome", "/outcome_by_request"):
        return f"commit-path:{name.lstrip('/')}"
    if name == "/read":
        c = q.get(x[4] // 64, 0)
        return f"read:{Q[c - 1] if 0 < c <= len(Q) else 'other'}"
    return name.lstrip("/")


tot, opened, secs_all, per_sec = collections.Counter(), collections.defaultdict(list), 0, []
for s in sorted(rows):
    V = sorted(rows[s])
    q = {x[3]: x[4] for x in V if x[1] == 26}
    E = [x for x in V if x[1] == 25 and T0 <= x[0] <= T1 and leader(x[0]) == s]
    if not E:
        continue
    secs = sorted({x[0] // 1000 for x in E})
    secs_all += len(secs)
    stall = collections.Counter()
    for x in V:
        if x[1] == 8 and T0 <= x[0] <= T1 and leader(x[0]) == s:
            a, b = x[0] - x[5], x[0]
            for sec in range(a // 1000, b // 1000 + 1):
                stall[sec] += max(0, min(b, (sec + 1) * 1000) - max(a, sec * 1000))
    by = collections.defaultdict(collections.Counter)
    for x in E:
        k = stream(x, q)
        tot[k] += 1; opened[k].append(x[5]); by[x[0] // 1000][k] += 1
    for b in secs:
        per_sec.append((sum(by[b].values()), stall[b], by[b], (s, b)))
    print(f"member {s}: led {len(secs)} s, {len(E) / len(secs):.0f} req/s, open p50 {pct([x[5] for x in E], .5)} "
          f"p90 {pct([x[5] for x in E], .9)}, stall {sum(stall[b] for b in secs) / len(secs) / 10:.0f}%")

n = sum(tot.values())
cp = sum(v for k, v in tot.items() if k.startswith("commit-path"))
print(f"\ncommit path in all: {cp / max(secs_all, 1):.1f}/s {cp / n:.1%}")
print(f"\nleader inbound by stream over {secs_all} leader-seconds: req/s, share, open at entry p50/p90")
for k, v in tot.most_common():
    print(f"  {k:22} {v / secs_all:7.1f}/s {v / n:6.1%}  open {pct(opened[k], .5)}/{pct(opened[k], .9)}")
print("\nstall share by leader req/s band, and each stream's req/s in the band")
keys = [k for k, _ in tot.most_common(6)]
print(f"  {'band':>11} {'s':>4} {'stall':>6}  " + " ".join(f"{k[:12]:>12}" for k in keys))
for lo, hi in ((0, 150), (150, 300), (300, 450), (450, 600), (600, 900), (900, 10 ** 9)):
    v = [p for p in per_sec if lo <= p[0] < hi]
    if v:
        print(f"  {lo:>5}-{hi:<5} {len(v):4} {sum(p[1] for p in v) / len(v) / 10:5.0f}%  "
              + " ".join(f"{sum(p[2][k] for p in v) / len(v):12.1f}" for k in keys))


def ranks(xs):
    o = sorted(range(len(xs)), key=lambda i: xs[i])
    r = [0.0] * len(xs)
    i = 0
    while i < len(o):
        j = i
        while j + 1 < len(o) and xs[o[j + 1]] == xs[o[i]]:
            j += 1
        for k in range(i, j + 1):
            r[o[k]] = (i + j) / 2
        i = j + 1
    return r


def spearman(a, b):
    if len(a) < 3:
        return float("nan")
    ra, rb = ranks(a), ranks(b)
    ma, mb = sum(ra) / len(ra), sum(rb) / len(rb)
    num = sum((x - ma) * (y - mb) for x, y in zip(ra, rb))
    den = (sum((x - ma) ** 2 for x in ra) * sum((y - mb) ** 2 for y in rb)) ** 0.5
    return num / den if den else float("nan")


# Which stream moves with the stall: Spearman of each stream's requests in second t with the stall in t (same second)
# and in t + 1 (leads by a second), over the leader's seconds. A stream that only follows the stall (retries,
# outcome asks) correlates at lag 0 and less at lead 1; a cause should lead.
per_sec.sort(key=lambda p: p[3])
print("\nSpearman with the stall ms: same second, and the stream one second before the stall")
groups = {"commit-path": lambda c: sum(v for k, v in c.items() if k.startswith("commit-path")),
          "raft:learner": lambda c: c["raft:learner"], "read:refs": lambda c: c["read:refs"],
          "raft:voter": lambda c: c["raft:voter"], "all": lambda c: sum(c.values())}
for name, f in groups.items():
    same = spearman([f(p[2]) for p in per_sec], [p[1] for p in per_sec])
    pairs = [(f(a[2]), b[1]) for a, b in zip(per_sec, per_sec[1:]) if a[3][0] == b[3][0] and b[3][1] == a[3][1] + 1]
    lead = spearman([x for x, _ in pairs], [y for _, y in pairs])
    print(f"  {name:14} {same:5.2f}  {lead:5.2f}")
