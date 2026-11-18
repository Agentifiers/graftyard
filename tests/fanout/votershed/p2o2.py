#!/usr/bin/env python3
"""Point 2 of the voter-shed review (voter-shed-design.md, "Point 2"): m47 leader open requests by class, and the
leader->follower round trip split. Needs /tmp/shed/rows.pkl (prep.py). numpy/scipy: uv run -q --with numpy --with scipy."""
exec(open(__import__('os').path.join(__import__('os').path.dirname(__file__),'p2d.py')).read().split('# Little')[0].split("d=collections.defaultdict(list)\nfor s in rows:")[0])
import numpy as np
from scipy.optimize import nnls
# 1) skew-free round trip (leader clock both ends) minus callee residence, by gate state; share of summed excess over 300 ms baseline
# 2) leader open by class at the cut-off: 3-class NNLS over 1 s bins, and read residence bound from low-commit bins
for s in sorted(rows):
    E=[x for x in ent[s] if T0<=x[0]<=T1 and leader(x[0])==s]
    if len(E)<5000: continue
    lft={(x[2],x[3]):x[0] for x in rows[s] if x[1]==24}
    raft=[(x[0],lft[(x[2],x[3])]) for x in E if x[4]%64==1 and x[3] and (x[2],x[3]) in lft]
    ra=sorted(a for a,b in raft); rb=sorted(b for a,b in raft)
    ro=lambda t:B.bisect_right(ra,t)-B.bisect_right(rb,t)
    C=[('/read',),('/suspect',),('/commit','/outcome','/outcome_by_request')]
    sec=collections.defaultdict(collections.Counter); op=collections.defaultdict(list)
    for x in E:
        r=route(x[4]%64)
        for i,c in enumerate(C):
            if r in c: sec[x[0]//1000][i]+=1
        op[x[0]//1000].append(x[5]-ro(x[0]))
    bins=sorted(op); W=6
    A=np.array([[sum(sec[b-k][i] for k in range(W))/W for i in range(3)] for b in bins]); y=np.array([np.mean(op[b]) for b in bins])
    D,_=nnls(A,y); fit=A@D; r2=1-((y-fit)**2).sum()/((y-y.mean())**2).sum()
    hi=y>=64; sh=(A[hi]*D).sum(0)/max(1e-9,(A[hi]*D).sum())
    low=[op_/max(1,a[0]) for a,op_ in zip(A,y) if a[2]<10 and a[0]>=50]
    rs=np.mean([ro(x[0])/max(1,x[5]) for x in E if x[5]>=64])
    print(f'member {s}: fit R2 {r2:.2f}, residence s read {D[0]:.2f} suspect {D[1]:.2f} commit-family {D[2]:.2f}; non-/raft open at >=64 by class read {sh[0]:.0%} suspect {sh[1]:.0%} commit-family {sh[2]:.0%}; /raft {rs:.0%} of open (exact); read residence bound in low-commit bins p50 {(np.median(low) if low else float('nan')):.3f} s')
