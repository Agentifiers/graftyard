#!/usr/bin/env python3
"""BM-1's comparison: per test, a faulted conformance run's category (`results.tsv`, which report.py writes) against
the no-fault baseline's (`tests/m1/results/raw/conformance-20261101/results.tsv`). Every baseline pass must pass; the
script lists each test whose category differs, and the counts per version.

    python3 tests/m1/wp8/compare.py <faulted results.tsv> [baseline results.tsv]
"""
import collections
import pathlib
import sys

here = pathlib.Path(__file__).resolve()
run = pathlib.Path(sys.argv[1])
base = pathlib.Path(sys.argv[2]) if len(sys.argv) > 2 else here.parents[1] / "results/raw/conformance-20261101/results.tsv"


def load(p):
    out = {}
    for line in p.read_text().splitlines():
        f = line.split("\t")
        if len(f) >= 5:
            out[(f[0], f[1], int(f[2]))] = (f[4], f[3], f[5] if len(f) > 5 else "")
    return out


a, b = load(base), load(run)
versions = sorted({k[0] for k in b})
for v in versions:
    cnt = collections.Counter(c for k, (c, _, _) in b.items() if k[0] == v)
    bcnt = collections.Counter(c for k, (c, _, _) in a.items() if k[0] == v)
    print(f"git {v}: run {sum(cnt.values())} ({dict(cnt)}); baseline {sum(bcnt.values())} ({dict(bcnt)})")
lost = []
for k in sorted(b):
    was = a.get(k)
    now = b[k]
    if was is None:
        print(f"  not in the baseline: {k[0]} {k[1]} #{k[2]} {now[1]!r}: {now[0]} {now[2]}")
    elif was[0] != now[0]:
        print(f"  {k[0]} {k[1]} #{k[2]} {now[1]!r}: baseline {was[0]}, now {now[0]} {now[2]}")
        if was[0] == "pass":
            lost.append(k)
for k in sorted(a):
    if k[0] in versions and k not in b:
        print(f"  did not run: {k[0]} {k[1]} #{k[2]} {a[k][1]!r} (baseline {a[k][0]})")
        if a[k][0] == "pass":
            lost.append(k)
print(f"baseline passes that did not pass: {len(lost)}")
