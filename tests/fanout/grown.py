import bisect, json, lzma, sys
# grown.py TRACE.jsonl[.xz] LOAD_START_MS SECONDS [SITE]: soft-grow's verdict lines from a leader's trace
# (`decisions/DECISIONS.md`, "`soft-grow`", its flip). Uses the leader site's rows (default 0) and the followers'.
#   Cuts (`Cut`, kind 29): how many ended `Soft` within ENTRY_SOFT and how many grew past it (soft bytes over
#   ENTRY_SOFT), by soft bytes; and every cut's taken commands.
#   Entries: each leader `AppendQueued` (kind 12) to a voter, keyed by its last index, takes the size of the cut just
#   before it, so each entry is "grown" (over ENTRY_SOFT) or not.
#   Confirms: `Confirm` (kind 10, `index` the applied index it covered, `ms` its wall) and `Sync` (kind 31, `ms` its
#   wall, `index` statements and bytes) on every voter site, split by whether the entry they cover is grown. A Sync
#   row is joined to the Confirm row on the same site at the same time (both are logged when `sync()` resolves).
#   Far-voter misses: `AppendEnded` (kind 14) with `index` 1 (the call failed), by peer, and how many of those calls
#   carried a grown entry (joined by round to `AppendQueued`'s `batch`).
#   Also counts calls answered past CALL_DEADLINE (1 s), which the driver reports as `deadline` failures (m63: 3 and 4
#   in both).
ENTRY_SOFT = 100_000
path, t0 = sys.argv[1], int(sys.argv[2])
t1 = t0 + 1000 * int(sys.argv[3])
LEAD = int(sys.argv[4]) if len(sys.argv) > 4 else 0
seen = set()
cuts, queued, ended, confirms, syncs = [], {}, [], [], []
for l in (lzma.open if path.endswith('.xz') else open)(path, 'rt'):
    d = json.loads(l)
    if 'rows' not in d or 'learner' in d:
        continue
    site = d.get('site')
    for r in d['rows']:
        k = (site,) + tuple(r)
        if k in seen or not t0 - 30_000 <= r[0] < t1 + 30_000:
            continue
        seen.add(k)
        at, kind, peer = r[0], r[1], r[2]
        b, i, ms = int(float(r[3])), int(float(r[4])), int(float(r[5]))
        if site == LEAD and kind == 29 and t0 <= at < t1:
            cuts.append((at, peer % 16, b & 0xfff, ms & 0x1fffff))
        elif site == LEAD and kind == 12 and peer < 192:
            queued.setdefault((peer, b), (at, i))
        elif site == LEAD and kind == 14 and peer < 192:
            ended.append((at, peer, b, i, ms))
        elif kind == 10 and t0 <= at < t1:
            confirms.append((site, at, i, ms))
        elif kind == 31 and t0 <= at < t1:
            syncs.append((site, at, ms, i & 0xfffff, i >> 20))
cuts.sort()
ct = [c[0] for c in cuts]


def pc(xs, q):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(len(xs) * q))] if xs else None


soft = [c for c in cuts if c[1] == 1]
print(f'{len(cuts)} cuts in the load; ends Soft {len(soft)}, Empty {sum(1 for c in cuts if c[1] == 0)}')
print(f'  Soft at or under ENTRY_SOFT: {sum(1 for c in soft if c[3] <= ENTRY_SOFT)}; grown past it: '
      f'{sum(1 for c in cuts if c[3] > ENTRY_SOFT)} (of which Soft {sum(1 for c in soft if c[3] > ENTRY_SOFT)})')
print(f'  cut soft bytes p50 {pc([c[3] for c in cuts], .5)} p90 {pc([c[3] for c in cuts], .9)} '
      f'p99 {pc([c[3] for c in cuts], .99)} max {max((c[3] for c in cuts), default=0)}')
# Entry sizes, by last index.
size = {}
for (peer, rnd), (at, last) in queued.items():
    j = bisect.bisect_right(ct, at) - 1
    if j >= 0 and t0 <= at < t1:
        size.setdefault(last, cuts[j][3])
idx = sorted(size)


def grown_at(applied):
    j = bisect.bisect_left(idx, applied)
    return None if j >= len(idx) else size[idx[j]] > ENTRY_SOFT


for name, rows in (('leader', [c for c in confirms if c[0] == LEAD]), ('followers', [c for c in confirms if c[0] != LEAD])):
    g = [c[3] for c in rows if grown_at(c[2]) is True]
    s = [c[3] for c in rows if grown_at(c[2]) is False]
    print(f'confirm wall, {name}: grown n {len(g)} p50 {pc(g, .5)} p99 {pc(g, .99)} max {max(g, default=None)}, '
          f'>=1 s {sum(1 for x in g if x >= 1000)}; <=100 KB n {len(s)} p50 {pc(s, .5)} p99 {pc(s, .99)} '
          f'max {max(s, default=None)}, >=1 s {sum(1 for x in s if x >= 1000)}')
# Sync rows split by their own bytes (the transaction's bytes; a grown entry's transaction is over 100 KB).
for name, rows in (('leader', [s for s in syncs if s[0] == LEAD]), ('followers', [s for s in syncs if s[0] != LEAD])):
    g = [s[2] for s in rows if s[4] > ENTRY_SOFT]
    sm = [s[2] for s in rows if s[4] <= ENTRY_SOFT]
    print(f'sync wall by transaction bytes, {name}: >100 KB n {len(g)} p50 {pc(g, .5)} p99 {pc(g, .99)} '
          f'max {max(g, default=None)}, >=1 s {sum(1 for x in g if x >= 1000)}; <=100 KB n {len(sm)} p50 {pc(sm, .5)} '
          f'p99 {pc(sm, .99)} max {max(sm, default=None)}, >=1 s {sum(1 for x in sm if x >= 1000)}')
# Voter AppendEntries failures.
rounds = {(peer, rnd): last for (peer, rnd), (at, last) in queued.items()}
for peer in sorted({e[1] for e in ended}):
    es = [e for e in ended if e[1] == peer and t0 <= e[0] < t1]
    fails = [e for e in es if e[3] == 1]
    gf = [e for e in fails if (peer, e[2]) in rounds and size.get(rounds[(peer, e[2])], 0) > ENTRY_SOFT]
    calls_g = [e for e in es if (peer, e[2]) in rounds and size.get(rounds[(peer, e[2])], 0) > ENTRY_SOFT]
    print(f'voter {peer}: {len(es)} AppendEntries ended, {len(fails)} failed ({len(gf)} of them carrying a grown '
          f'entry; {len(calls_g)} calls carried one), call p99 {pc([e[4] for e in es], .99)} ms, '
          f'grown-entry calls p99 {pc([e[4] for e in calls_g], .99)} ms; answered past CALL_DEADLINE (1 s) '
          f'{sum(1 for e in es if e[4] >= 1000)}, of them carrying a grown entry {sum(1 for e in calls_g if e[4] >= 1000)}')
