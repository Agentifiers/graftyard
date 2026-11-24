#!/usr/bin/env python3
"""latesuspect.py TSAVE TERMS -> a B0-33 run's withholding replayed on main's SUSPECT rule and on rule (i) at several deadlines.
A report counts as the code counts it: verdict `promised` (4), `about` the member this voter last accepted an append
from (`AppendStepped` 17, `peer`), and colo not UNKNOWN. Rule (i) adds: the gap since that last append is at least
the deadline, judged only where the voter's trace has no hole over 300 ms across the gap (a ring-loss guard, so the
count is a lower bound on (i)'s withholding). A higher term clears the window and the bar. Prints per voter the episodes
and seconds withheld, the seconds two followers withheld at once, the first such moment, the terms in which both would
lapse (10 s at a stretch), and per term its length and the first two-voter withholding 3 s or more into it.
  python3 tests/fanout/latesuspect.py tests/fanout/raw/tsave-m44b33.jsonl.xz tests/fanout/raw/terms-m44b33.jsonl"""
import bisect, collections, json, lzma, os, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from tracerows import load

rows = load(sys.argv[1])
WINDOW, BAR = 10_000, 20_000
IV = collections.defaultdict(dict)
for s in sorted(rows):
    allt = sorted(x[0] for x in rows[s])
    holes = [(a, b) for a, b in zip(allt, allt[1:]) if b - a > 300]
    hs = [h[0] for h in holes]

    def dense(a, b):
        i = bisect.bisect_left(hs, a)
        j = bisect.bisect_left(hs, b)
        return i == j and not (i and holes[i - 1][1] > a)

    app = sorted((x[0], x[2]) for x in rows[s] if x[1] == 17)
    at_ = [t for t, _ in app]
    reps = sorted(x for x in rows[s] if x[1] == 28 and x[5] == 4 and x[4] != 0)
    for dl in (0, 1000, 2000, 3000, 5000):
        seen, until, total, eps, ivs, term = {}, 0, 0, 0, [], 0
        for x in reps:
            t = x[0]
            if x[3] > term:
                term, seen = x[3], {}
                if until > t:
                    ivs[-1] = (ivs[-1][0], t)
                    until = t
            if x[3] < term:
                continue
            i = bisect.bisect_right(at_, t)
            if not i or app[i - 1][1] != x[2]:
                continue
            last = app[i - 1][0]
            if dl and not (t - last >= dl and dense(last, t)):
                continue
            seen = {k: v for k, v in seen.items() if v >= t - WINDOW}
            seen[x[4]] = t
            if len(seen) >= 2:
                eps += t >= until
                total += max(0, t + BAR - max(t, until))
                until = max(until, t + BAR)
                ivs.append((t, t + BAR))
        merged = []
        for a, b in sorted(ivs):
            if merged and a <= merged[-1][1]:
                merged[-1] = (merged[-1][0], max(merged[-1][1], b))
            else:
                merged.append((a, b))
        IV[dl][s] = merged
        print(f"voter {s} rule {'main' if dl == 0 else f'(i)>={dl}ms'}: {eps} episodes, {total / 1000:.0f} s withheld")
for dl, per in IV.items():
    ev = sorted(e for ivs in per.values() for a, b in ivs for e in ((a, 1), (b, -1)))
    n = both = 0
    last = None
    for t, d in ev:
        if n >= 2:
            both += t - last
        n += d
        last = t
    print(f"rule {'main' if dl == 0 else f'(i)>={dl}ms'}: two or more voters withholding at once {both / 1000:.0f} s")
# The first moment two voters withhold at once, against the first report (load start), per rule.
r0 = min(x[0] for s in rows for x in rows[s] if x[1] == 28)
for dl, per in IV.items():
    ev = sorted(e for ivs in per.values() for a, b in ivs for e in ((a, 1), (b, -1)))
    n, first = 0, None
    for t, d in ev:
        n += d
        if n >= 2 and first is None:
            first = t
    print(f"rule {'main' if dl == 0 else f'(i)>={dl}ms'}: first two-voter withholding {(first - r0) / 1000 if first else None} s after the first report")
# Per real term: would the rule lapse both followers' promises inside that term? A lapse needs both followers to
# withhold for ELECTION_AFTER (10 s) at a stretch, starting after the term's first leader sample.
import datetime
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
first = {}
for l in open(sys.argv[2]):
    if '"members"' not in l:
        continue
    r = json.loads(l)
    for m in r["members"]:
        if m[1] == "leader" and m[2] not in first:
            first[m[2]] = iso(r["at"])
terms = sorted(first.items())
for dl, per in IV.items():
    ev = sorted(e for ivs in per.values() for a, b in ivs for e in ((a, 1), (b, -1)))
    stretches, n, s0 = [], 0, None
    for t, d in ev:
        was = n >= 2
        n += d
        if not was and n >= 2:
            s0 = t
        elif was and n < 2:
            stretches.append((s0, t))
    hit = []
    for (tn, a), nxt in zip(terms, terms[1:] + [(None, 1 << 62)]):
        b = nxt[1]
        if any(a <= x < b and min(y, b) - x >= 10_000 for x, y in stretches):
            hit.append(tn)
    print(f"rule {'main' if dl == 0 else f'(i)>={dl}ms'}: terms whose followers would both lapse inside the term "
          f"{len(hit)} of {len(terms)}: {hit}")
# Per term: the first moment both followers withhold at once, counted from 3 s after the term's first leader sample
# (a new leader's first appends are late by nature), against the term's length.
for dl in (0, 1000):
    per = IV[dl]
    ev = sorted(e for ivs in per.values() for a, b in ivs for e in ((a, 1), (b, -1)))
    starts, n = [], 0
    for t, d in ev:
        was = n >= 2
        n += d
        if not was and n >= 2:
            starts.append(t)
    out = []
    for (tn, a), nxt in zip(terms, terms[1:] + [(None, None)]):
        b = nxt[1] or a + 60_000
        f = next((t for t in starts if a + 3_000 <= t < b), None)
        out.append(f"t{tn}:{(b - a) / 1000:.0f}s/{'-' if f is None else f'{(f - a) / 1000:.1f}'}")
    print(f"rule {'main' if dl == 0 else '(i)>=1000ms'} per term (length / first two-voter withholding after start): {' '.join(out)}")
