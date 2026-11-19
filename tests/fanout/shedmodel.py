#!/usr/bin/env python3
"""shedmodel.py TSAVE A1_SAMPLES A1_ROW -> `shed-retry-2s`'s model of a write's shed-retry budget, fitted to one B0-33
row (B0-33 m56: `raw/tsave-m56b33.jsonl.xz`, `tests/am1/raw/a1-samples-b0-33-mwn8854t.jsonl.gz`, `raw/a1-b0-33-m56.jsonl`).

1. Per 10 s bin of the row's `seconds`: tier-read writes by outcome (served, 503, other) from a1's samples, and every
   member's edge-originated `/read` (Entered 25, route 2) from the trace. A least-squares fit gives member reads per
   served write and per write that ended 503.
2. Per 20 s bin: the share of writes ending 503 against the member `/read` rate L, fitted as j(L) = 1 - e^(-(L-L0)/K).
3. A Monte Carlo of `graftyard_lookup::retry_shed` for a write whose every pass sheds: SPREAD_TRIES members a pass,
   each shedding `late` (FOLLOWER_WAIT, 1 s) with the row's share, else at once; scaled so the row's own budget gives
   the fitted reads per 503 write.
4. For each budget, the fixed point of L = W (r_ok + j(L)(r_shed - r_ok)) + ref reads, started from a full jam (the
   storm) and from none, and again with L0 +- 40 and K +- 30%.
The fit, the late share (LATE) and a pass's member count are the model's assumptions [inference]."""
import collections, gzip, json, lzma, math, random, sys

TS, SAMP, ROW = sys.argv[1:4]
LATE = float(sys.argv[4]) if len(sys.argv) > 4 else 0.62
row = [d for d in map(json.loads, open(ROW)) if 'offered' in d][-1]
S = [s for s in (json.loads(l) for l in gzip.open(SAMP, 'rt')) if isinstance(s, list)]
W_KIND = lambda k: k.startswith('side:') or k in ('push:agent', 'fast-user', 'push:human')
T0 = min(t for t, k, _, _ in S if W_KIND(k))
T1 = T0 + row['seconds'] * 1000


def bins(width):
    ok, bad, oth, ref, rd = (collections.Counter() for _ in range(5))
    for t, k, _, c in S:
        if not T0 <= t < T1:
            continue
        b = (t - T0) // width
        if k == 'read:ref':
            ref[b] += 1
        elif W_KIND(k):
            (bad if '503' in c else ok if c in ('ok', '200', '201', '204') else oth)[b] += 1
    for b, n in READS.items():
        rd[b * 1000 // width] += n
    return [(ok[b], bad[b], oth[b], ref[b], rd[b]) for b in range((T1 - T0) // width)]


READS, seen = collections.Counter(), set()
for line in (lzma.open if TS.endswith('.xz') else open)(TS, 'rt'):
    r = json.loads(line)
    if 'rows' not in r:
        continue
    who = ('l', r['learner']) if 'learner' in r else ('v', r['site'])
    for x in r['rows']:
        if x[1] == 25 and x[4] % 64 == 2 and T0 <= x[0] < T1 and (who, *x) not in seen:
            seen.add((who, *x))
            READS[(x[0] - T0) // 1000] += 1


def lsq(rows):
    n = len(rows[0]) - 1
    m = [[sum(r[i] * r[j] for r in rows) for j in range(n)] + [sum(r[i] * r[-1] for r in rows)] for i in range(n)]
    for i in range(n):
        p = max(range(i, n), key=lambda k: abs(m[k][i]))
        m[i], m[p] = m[p], m[i]
        for k in range(n):
            if k != i:
                f = m[k][i] / m[i][i]
                m[k] = [a - f * b for a, b in zip(m[k], m[i])]
    return [m[i][n] / m[i][i] for i in range(n)]


r_ok, r_shed, r_oth = lsq([(o, f, x, rd) for o, f, x, _, rd in bins(10_000)])
b20 = bins(20_000)
L = [r[4] / 20 for r in b20]
J = [r[1] / max(1, r[0] + r[1]) for r in b20]
W = sum(r[0] + r[1] + r[2] for r in b20) / row['seconds']
REF = sum(r[3] for r in b20) / row['seconds']
_, L0, K = min((sum((max(0, 1 - math.exp(-(l - l0) / k)) - j) ** 2 for l, j in zip(L, J)), l0, k)
               for l0 in range(150, 400, 5) for k in range(100, 900, 10))

random.seed(7)
member = lambda: 1000 + random.uniform(50, 200) if random.random() < LATE else random.uniform(40, 250)


def shed_wait(n, named=250):
    cap = min(250 * (1 << n), 2000)
    return max(cap // 2 + int(random.random() * (cap // 2)), named)


def reads_per_jammed(budget, max_passes=99, runs=20_000):
    tot = 0
    for _ in range(runs):
        t = n = p = 0
        while True:
            t += sum(member() for _ in range(3))
            tot += 3
            p += 1
            ms = shed_wait(n)
            n += 1
            if p >= max_passes or t + ms >= budget:
                break
            t += ms
    return tot / runs


scale = r_shed / reads_per_jammed(row.get('shedRetryForMs', 4000))


def fixed(r, l0, k, start):
    j = start
    for _ in range(500):
        rate = W * (r_ok + j * (r - r_ok)) + REF
        j = 0.5 * j + 0.5 * max(0.0, 1 - math.exp(-(rate - l0) / k))
    return j, W * (r_ok + j * (r - r_ok)) + REF


print(f"reads per write: served {r_ok:.2f}, 503 {r_shed:.2f}, other {r_oth:.2f}; W {W:.1f}/s, ref reads {REF:.1f}/s; "
      f"shed share 1-exp(-(L-{L0})/{K})")
for name, budget, mp in (("4 s", 4000, 99), ("one retry pass, 4 s", 4000, 2), ("3.5 s", 3500, 99), ("3 s", 3000, 99),
                         ("2.5 s", 2500, 99), ("2 s", 2000, 99), ("1.5 s", 1500, 99), ("no retry", 0, 1)):
    r = reads_per_jammed(budget, mp) * scale
    js, ls = fixed(r, L0, K, 1.0)
    jc, lc = fixed(r, L0, K, 0.0)
    alt = [fixed(r, L0 + d, K * f, 1.0)[0] for d, f in ((-40, 1), (40, 1), (0, 0.7), (0, 1.3))]
    print(f"{name:20s} reads/jammed write {r:4.2f} | storm: share {js:.2f}, /read per request {ls / W:.2f}, "
          f"503 {W * js:4.0f}/s | calm: /read per request {lc / W:.2f} | storm share with fit moved "
          + " ".join(f"{a:.2f}" for a in alt))
