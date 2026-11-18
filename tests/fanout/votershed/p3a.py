#!/usr/bin/env python3
"""(c) design evidence (tests/am1/results/c-design.md): m47 leader stalls, releases and the round trip. Needs
/tmp/shed/rows.pkl (prep.py); uv run -q --with numpy --with scipy."""
exec(open(__import__('os').path.join(__import__('os').path.dirname(__file__),'p2d.py')).read().split('# Little')[0].split("d=collections.defaultdict(list)\nfor s in rows:")[0])
# (1) gate truth: for sends issued while a confirm was pending (JS view), when did they enter the follower relative to
# that confirm's JS resolution? arrival - resolution, skew-corrected. Wire one-way baseline from quiet sends.
quiet=[]; held=[]
for s in rows:
    for x in rows[s]:
        if x[1]==23 and T0<=x[0]<=T1 and (s,x[3]) in raft_in and leader(x[0])==s and (s,x[2]) in skew:
            c,at=raft_in[(s,x[3])]; arr=at-skew[(s,x[2])]
            res=[b for a,b in pend[s] if a<=x[0]<b]
            if res: held.append((arr-max(res), max(res)-x[0], open_at(s,x[0])))
            elif open_at(s,x[0])<25: quiet.append(arr-x[0])
print('quiet one-way (gate open, open<25) p10/p50/p90',pct(quiet,.1),pct(quiet,.5),pct(quiet,.9),'n',len(quiet))
for lo,hi in ((0,64),(64,300),(300,10**9)):
    v=[h for h in held if lo<=h[2]<hi]
    a=[h[0] for h in v]
    print(f' gate-closed sends open {lo}-{hi}: n {len(v)}; entered follower minus JS confirm resolution p10/p50/p90 {pct(a,.1)}/{pct(a,.5)}/{pct(a,.9)}; share entered BEFORE the JS resolution {sum(1 for x in a if x<0)/max(1,len(a)):.0%}; JS gate left at send p50 {pct([h[1] for h in v],.5)}')
