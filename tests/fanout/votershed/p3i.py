#!/usr/bin/env python3
"""(c) design evidence (tests/am1/results/c-design.md): m47 leader stalls, releases and the round trip. Needs
/tmp/shed/rows.pkl (prep.py); uv run -q --with numpy --with scipy."""
exec(open(__import__('os').path.join(__import__('os').path.dirname(__file__),'p2a.py')).read().split('for s in sorted(rows):')[0])
import bisect as B, random
random.seed(1)
for s in sorted(rows):
    V=rows[s]
    sts=[(x[0]-x[5],x[5]) for x in V if x[1]==8 and T0<=x[0]<=T1 and leader(x[0])==s and x[5]>=300]
    if not sts: continue
    ap=sorted(x[0] for x in V if x[1]==4); cf=sorted(x[0] for x in V if x[1]==10)
    rel=sorted(x[0] for x in V if x[1] in (24,))  # CallLeft: a /raft answer returned
    near=lambda L,t,w:B.bisect_right(L,t)-B.bisect_left(L,t-w)>0
    lead=[t for t in range(T0,T1,250) if leader(t)==s]
    rnd=[random.choice(lead) for _ in range(2000)]
    for name,L in (('Applied (commit released waiters)',ap),('Confirm resolved',cf)):
        a=sum(near(L,t,100) for t,_ in sts)/len(sts); b=sum(near(L,t,100) for t in rnd)/len(rnd)
        print(f'member {s}: {len(sts)} stalls >=300 ms; {name} within 100 ms before stall start: {a:.0%} vs {b:.0%} at random leader times')
