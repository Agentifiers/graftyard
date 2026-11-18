#!/usr/bin/env python3
"""syncload.py TSAVE FROM TO TERMS -> F92's test: does a member's own inbound load make its storage confirms slow, or
are the slow syncs storage luck that happens to sit on the leader? Per member and second: inbound requests entered
(Entered 25), and the time `sync()` was busy that second, from `Confirm` rows (10, `ms` the sync's duration, stamped
at its end). Prints, for every member, the slow-sync rate by role and by inbound band, and the Spearman of each
second's sync-busy ms against its inbound, over all members and seconds and within each role. Storage luck predicts
no inbound dependence within a role and the same slow rate for a member in either role at the same inbound;
load predicts slow syncs rise with inbound on every member. TERMS from terms.mjs gives each second's role.
  python3 tests/fanout/syncload.py tests/fanout/raw/tsave-m52b33.jsonl.xz 2026-11-18T14:44:21Z 2026-11-18T14:49:21Z \\
    tests/fanout/raw/terms-m52b33.jsonl [SHIFT_MS]
SHIFT_MS moves TERMS onto the trace's clock for runs before srvclock (m50 +7310, m51 +7440)."""
import bisect, collections, datetime, json, lzma, sys

P, F, T, TERMS = sys.argv[1:5]
SHIFT = int(sys.argv[5]) if len(sys.argv) > 5 else 0
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
T0, T1 = iso(F), iso(T)
rows = collections.defaultdict(set)
for line in (lzma.open if P.endswith(".xz") else open)(P, "rt"):
    r = json.loads(line)
    if "rows" in r and "learner" not in r:
        rows[r["site"]].update(tuple(x) for x in r["rows"] if x[1] in (10, 16, 25))
samples = [(iso(r["at"]) + SHIFT, {m[0]: m[1] for m in r["members"]}) for r in map(json.loads, open(TERMS)) if r.get("members")]
st = [t for t, _ in samples]


def role(t, s):
    i = max(0, bisect.bisect_right(st, t) - 1)
    return samples[i][1].get(s, "?")


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


secs = []  # (member, role, inbound, sync-busy ms, syncs >= 1 s)
for s in sorted(rows):
    inb, busy, slow = collections.Counter(), collections.Counter(), collections.Counter()
    for x in rows[s]:
        if not T0 <= x[0] < T1:
            continue
        if x[1] == 25:
            inb[x[0] // 1000] += 1
        elif x[1] == 10:
            a, b = x[0] - x[5], x[0]
            for sec in range(a // 1000, b // 1000 + 1):
                busy[sec] += max(0, min(b, (sec + 1) * 1000) - max(a, sec * 1000))
            if x[5] >= 1000:
                slow[x[0] // 1000] += 1
    for sec in range(T0 // 1000, T1 // 1000):
        secs.append((s, role(sec * 1000 + 500, s), inb[sec], busy[sec], slow[sec]))

flushes = {s: sorted(x for x in v if x[1] == 16) for s, v in rows.items()}
print("per member and role: seconds, inbound req/s, syncs >= 1 s per minute, sync-busy share")
for (s, r), v in sorted(collections.defaultdict(list, {k: [x for x in secs if (x[0], x[1]) == k] for k in {(x[0], x[1]) for x in secs}}).items()):
    n = len(v)
    print(f"  member {s} {r:9} {n:4} s  {sum(x[2] for x in v) / n:6.0f} req/s  {60 * sum(x[4] for x in v) / n:5.1f}/min  "
          f"{sum(x[3] for x in v) / n / 10:4.0f}%")
print("slow syncs per minute by inbound band (all members, all roles; then followers only)")
for lo, hi in ((0, 25), (25, 100), (100, 200), (200, 400), (400, 700), (700, 10 ** 9)):
    v = [x for x in secs if lo <= x[2] < hi]
    f = [x for x in v if x[1] == "follower"]
    l = [x for x in v if x[1] == "leader"]
    if v:
        print(f"  {lo:>4}-{hi:<6} all {len(v):4} s {60 * sum(x[4] for x in v) / len(v):5.1f}/min"
              + (f" | leader {len(l):4} s {60 * sum(x[4] for x in l) / len(l):5.1f}/min" if l else "")
              + (f" | follower {len(f):4} s {60 * sum(x[4] for x in f) / len(f):5.1f}/min" if f else ""))
print("Spearman, a second's sync-busy ms against its inbound (and against the inbound one second before)")
for name, v in (("all", secs), ("leader", [x for x in secs if x[1] == "leader"]),
                ("follower", [x for x in secs if x[1] == "follower"])):
    lag = [(a[2], b[3]) for a, b in zip(v, v[1:]) if a[0] == b[0]]
    print(f"  {name:9} n {len(v):4}  same {spearman([x[2] for x in v], [x[3] for x in v]):5.2f}  "
          f"lead {spearman([x for x, _ in lag], [y for _, y in lag]):5.2f}")

# The other candidate: what one sync writes. Each Confirm is paired with the deferred transaction (Flushed 16, `index`
# its statements) that started within 50 ms before the sync began; Flushed rows are the leader's deferred batches.
print("a sync's duration against the statements of the transaction it confirmed (paired syncs; >= 1 s share, p50 ms)")
for s in sorted(rows):
    fl = [x for x in flushes[s] if T0 <= x[0] < T1]
    ft = [x[0] for x in fl]
    pairs = []
    for c in (x for x in rows[s] if x[1] == 10 and T0 <= x[0] < T1):
        start = c[0] - c[5]
        i = bisect.bisect_right(ft, start + 2) - 1
        if i >= 0 and start - ft[i] <= 50:
            pairs.append((fl[i][4], c[5], role(start, s)))
    if len(pairs) < 20:
        continue
    bands = collections.defaultdict(list)
    for n, d, _ in pairs:
        bands[min(n // 250 * 250, 1000)].append(d)
    print(f"  member {s}: {len(pairs)} paired, Spearman {spearman([p[0] for p in pairs], [p[1] for p in pairs]):5.2f}; "
          + "; ".join(f"{k}+ stmts n {len(v)} {100 * sum(1 for d in v if d >= 1000) / len(v):.0f}% p50 {sorted(v)[len(v) // 2]}"
                      for k, v in sorted(bands.items())))
