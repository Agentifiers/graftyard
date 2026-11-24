#!/usr/bin/env python3
"""vsplit.py PUSHES: where an agent push's time went around validate, by the git cache site that answered it (G40, B0-33
h1A to h3A, `tests/am1/results/b033-h1-set.md`). PUSHES is a1's push log (`a1-pushes-b0-33-<run>.jsonl`), each row's
`timing` its `server-timing`: validate and its `v_walk`, `v_lock_wait` and `v_index` parts by `validate_site` and
`validate_tries`, the time before validate (front, gate and context), and the budget left at validate's end.
"""
import json,re,collections,sys
P=[json.loads(l) for l in open(sys.argv[1])]
def t(p,k):
    m=re.search(r'(?:^|, )'+k+r';dur=(-?[\d.]+)',p.get('timing') or '')
    return float(m[1]) if m else None
def pc(xs,q):
    xs=sorted(xs); return round(xs[min(len(xs)-1,int(q*len(xs)))]) if xs else None
def s(xs): return f"p50 {pc(xs,.5)} p90 {pc(xs,.9)} p99 {pc(xs,.99)} (n {len(xs)})"
for kind in ('agent','human'):
    A=[p for p in P if p['kind']==kind and t(p,'validate') is not None]
    print(f"== {kind} pushes with a validate: {len(A)}")
    by=collections.defaultdict(list)
    for p in A: by[t(p,'validate_site')].append(p)
    for site in sorted(by, key=lambda x: -1 if x is None else x):
        xs=by[site]
        v=[t(p,'validate') for p in xs]
        walk=[t(p,'v_walk') or 0 for p in xs]; lock=[t(p,'v_lock_wait') or 0 for p in xs]; idx=[t(p,'v_index') or 0 for p in xs]
        over=[t(p,'validate')-(t(p,'v_walk') or 0)-(t(p,'v_lock_wait') or 0)-(t(p,'v_index') or 0) for p in xs]
        tries=collections.Counter(int(t(p,'validate_tries') or 0) for p in xs)
        print(f" site {site}: validate {s(v)}")
        print(f"   v_walk {s(walk)}; v_lock_wait {s(lock)}; v_index {s(idx)}; outside walk/index/lock {s(over)}; tries {dict(tries)}")
    if kind!='agent': continue
    pre=[(t(p,'front') or 0)+(t(p,'gate') or 0)+(t(p,'context') or 0) for p in A]
    print(" before validate (front+gate+context):", s(pre))
    # budget left at the validate's end: `budget` (left at the first send, right after validate and admission) where sent,
    # 0 where `bound` refused at send
    left=collections.defaultdict(list); res=collections.defaultdict(collections.Counter)
    for p in A:
        site=t(p,'validate_site')
        b=t(p,'budget')
        if b is None and t(p,'bound') is not None: b=0
        if b is not None: left[site].append(b)
        res[site][p['result'] if p['result'] in ('ok','full') else 'other']+=1
    allb=[b for v in left.values() for b in v]
    print(" budget left at the validate's end (ms; bound refusals as 0):", s(allb), "share under 1 s", round(sum(b<1000 for b in allb)/len(allb),3))
    for site in sorted(left, key=lambda x: -1 if x is None else x):
        xs=left[site]
        print(f"   site {site}: {s(xs)}; zero (bound) {sum(b==0 for b in xs)}; under 1 s {sum(b<1000 for b in xs)}; results {dict(res[site])}")
    nov=[p for p in P if p['kind']=='agent' and t(p,'validate') is None]
    print(" agent pushes with no validate timing:", len(nov), collections.Counter(p['result'] for p in nov).most_common(4))
