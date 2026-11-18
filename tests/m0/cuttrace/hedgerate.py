#!/usr/bin/env python3
"""dep-hedge-adaptive's replay (`tests/m0/results/dep-hedge-adaptive.md`): the share of dependency reads a hedge
delay sends a second request for, and the extra reads a second on the identity leaders, for fixed delays and for the
per-chain p90 rule (`crates/lookup`, `dep_hedge_after`).

No run kept per-read times, so each kind's reads are a lognormal fitted to its recorded p50 and p99 (`/stats`
`split.deps:*`). Run 5's at-apply read samples are replayed directly by `freezesim.py`; pass its trace to add the
commit p99 under the adaptive rule:

    python3 tests/m0/cuttrace/hedgerate.py
    python3 tests/m0/cuttrace/hedgerate.py tests/fanout/raw/tsave-m2bb29e.jsonl.xz 1794990053053
"""
import contextlib
import io
import math
import os
import random
import runpy
import sys

Z90, Z99 = 1.2816, 2.3263
# kind: (p50, p99, reads); seconds of load. Sources in the results file.
RUNS = {
    'B0-29 run 5 (enam leader, enam identity)': (600, {'authz': (53, 230, 6138), 'tokens': (47, 246, 4142), 'names': (29, 165, 1121)}),
    'B0-31 m3fr12 up half (apac leader, enam identity)': (300, {'authz': (293, 725, 2205), 'tokens': (174, 524, 1609), 'names': (174, 470, 515)}),
    'B0-31 m3fr16 up half (apac leader, enam identity)': (300, {'authz': (317, 745, 2152), 'tokens': (187, 524, 1570), 'names': (166, 470, 503)}),
    'B0-32 m3fr17 (enam leader)': (180, {'authz': (78, 595, 1106), 'tokens': (50, 324, 750), 'names': (41, 250, 208)}),
}
FLOOR, CAP, WINDOW, MIN = 100, 1000, 64, 16


def fit(p50, p99):
    mu = math.log(p50)
    return mu, (math.log(p99) - mu) / Z99


def past(p50, p99, h):
    mu, s = fit(p50, p99)
    return 0.5 * (1 - math.erf((math.log(h) - mu) / (s * math.sqrt(2))))


def adaptive(p50, p99, n=20000, seed=5):
    """The per-chain rule over n reads of one kind: hedged share, and read p99 before and after."""
    mu, s = fit(p50, p99)
    r = random.Random(seed)
    win, hedged, raw, out = [], 0, [], []
    for _ in range(n):
        h = CAP if len(win) < MIN else max(FLOOR, min(CAP, sorted(win)[len(win) * 9 // 10]))
        a = math.exp(r.gauss(mu, s))
        x = min(a, h + math.exp(r.gauss(mu, s))) if a > h else a
        hedged += a > h
        win = (win + [x])[-WINDOW:]
        raw.append(a)
        out.append(x)
    raw.sort()
    out.sort()
    return hedged / n, raw[int(n * .99)], out[int(n * .99)]


for name, (secs, kinds) in RUNS.items():
    total = sum(k[2] for k in kinds.values())
    print(f'{name}: {total} reads in {secs} s, {total / secs:.1f}/s')
    for h in (150, 200):
        extra = {k: past(p50, p99, h) * n / secs for k, (p50, p99, n) in kinds.items()}
        share = sum(past(p50, p99, h) * n for p50, p99, n in kinds.values()) / total
        each = ', '.join(f'{k} {past(*kinds[k][:2], h):.0%}' for k in kinds)
        print(f'  fixed {h} ms: hedged {share:.0%} ({each}), +{sum(extra.values()):.1f} reads/s ({extra["authz"]:.1f} on authz)')
    for k, (p50, p99, n) in kinds.items():
        mu, s = fit(p50, p99)
        share, before, after = adaptive(p50, p99)
        print(f'  per-chain p90, {k}: delay about {max(FLOOR, min(CAP, math.exp(mu + Z90 * s))):.0f} ms, hedged {share:.1%}, '
              f'+{share * n / secs:.2f} reads/s, read p99 {before:.0f} -> {after:.0f}')

if len(sys.argv) == 3:
    here = os.path.dirname(os.path.abspath(__file__))
    sys.argv = [sys.argv[0], sys.argv[1], sys.argv[2]]
    with contextlib.redirect_stdout(io.StringIO()):
        g = runpy.run_path(os.path.join(here, 'freezesim.py'))
    D0, D, sim, rule, pc = g['D0'], g['D'], g['sim'], g['rules']['current: append + mean round - mean read'], g['pc']
    print(f'run 5 at-apply reads (read plus decide): n {len(D0)}, p50 {pc(D0, .5)}, p90 {pc(D0, .9)}, p99 {pc(D0, .99)}')
    for label, delay in (('main (1 s)', lambda w: CAP), ('fixed 150 ms', lambda w: 150),
                         ('per-chain p90', lambda w: CAP if len(w) < MIN else max(FLOOR, min(CAP, sorted(w)[len(w) * 9 // 10])))):
        r = random.Random(7)
        win, out, hedged = [], [], 0
        for _ in range(5000):
            h = delay(win)
            a = r.choice(D0)
            x = min(a, h + r.choice(D0)) if a > h else a
            hedged += a > h
            win = (win + [x])[-WINDOW:]
            out.append(x)
        D[:] = out
        res = [sim(rule, z) for z in (1, 2, 3)]
        print(f'  {label}: hedged {hedged / 5000:.1%}, read p99 {pc(out, .99)}, '
              f'commit p50 {sum(o[0] for o in res) // 3} p99 {sum(o[1] for o in res) // 3}')
