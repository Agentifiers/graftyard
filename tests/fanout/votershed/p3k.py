#!/usr/bin/env python3
"""(c) design evidence (tests/am1/results/c-design.md): m47 leader stalls, releases and the round trip. Needs
/tmp/shed/rows.pkl (prep.py); uv run -q --with numpy --with scipy."""
exec(open(__import__('os').path.join(__import__('os').path.dirname(__file__),'p2a.py')).read().split('for s in sorted(rows):')[0])
import bisect as B
tot=collections.Counter()
for s in sorted(rows):
    V=rows[s]
    sts=[(x[0]-x[5],x[5]) for x in V if x[1]==8 and T0<=x[0]<=T1 and leader(x[0])==s and x[5]>=100]
    if not sts: continue
    ap=sorted(x[0] for x in V if x[1]==4); cf=sorted(x[0] for x in V if x[1]==10)
    near=lambda L,t,w:B.bisect_right(L,t+5)-B.bisect_left(L,t-w)>0
    c=collections.Counter()
    for t,ms in sts:
        k='after apply or confirm' if (near(ap,t,100) or near(cf,t,100)) else 'other'
        c[k]+=ms; tot[k]+=ms
    print(f'member {s}: stalls >=100 ms {len(sts)}, {sum(m for _,m in sts)/1000:.1f} s; share of stall time starting within 100 ms after an apply or confirm resolution {c["after apply or confirm"]/sum(c.values()):.0%}')
print(f'all leaders: {tot["after apply or confirm"]/sum(tot.values()):.0%} of {sum(tot.values())/1000:.1f} s')
