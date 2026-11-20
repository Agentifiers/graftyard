#!/usr/bin/env python3
"""growpool.py NAME=SAMPLES.json ... -> soft-grow's flip lines (`decisions/DECISIONS.md`, "`soft-grow`") per row and
pooled over every row given, from `grown.py`'s SAMPLES output: leader confirm p99 and counts of 1 s or more, grown
(entries over ENTRY_SOFT) against 100 KB or less; leader sync p99 by transaction bytes; and each voter's calls
answered past CALL_DEADLINE (1 s), grown against small, as a rate per call. A row with no grown samples (main's flat
rule, such as m62) prints its 100 KB-or-less lines only and is left out of the pool."""
import json, sys


def pc(xs, q):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(len(xs) * q))] if xs else None


def over(xs):
    return sum(1 for x in xs if x >= 1000)


def line(name, d):
    out = []
    for k in ('confirm:leader', 'sync:leader'):
        g, s = d[k]['grown'], d[k]['small']
        rate = lambda xs: f"{100 * over(xs) / len(xs):.2f}%" if xs else "-"
        out.append(f"  {k}: grown n {len(g)} p99 {pc(g, .99)} >=1 s {over(g)} ({rate(g)}); <=100 KB n {len(s)} "
                   f"p99 {pc(s, .99)} >=1 s {over(s)} ({rate(s)})")
    for k in sorted(x for x in d if x.startswith('calls:')):
        g, s = d[k]['grown'], d[k]['small']
        rg = f"{100 * over(g) / len(g):.1f}%" if g else "-"
        rs = f"{100 * over(s) / len(s):.1f}%" if s else "-"
        out.append(f"  voter {k[6:]}: past CALL_DEADLINE grown {over(g)} of {len(g)} ({rg}); small {over(s)} of {len(s)} ({rs})")
    print(name)
    print('\n'.join(out))


rows = [(a.split('=', 1)[0], json.load(open(a.split('=', 1)[1]))) for a in sys.argv[1:]]
pool = {}
for name, d in rows:
    line(name, d)
    if not d['confirm:leader']['grown']:
        continue
    for k, v in d.items():
        p = pool.setdefault(k, {'grown': [], 'small': []})
        p['grown'] += v['grown']
        p['small'] += v['small']
if pool:
    line('pooled (rows with grown entries)', pool)
