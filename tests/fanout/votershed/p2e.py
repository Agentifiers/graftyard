#!/usr/bin/env python3
"""Point 2 of the voter-shed review (voter-shed-design.md, "Point 2"): m47 leader open requests by class, and the
leader->follower round trip split. Needs /tmp/shed/rows.pkl (prep.py). numpy/scipy: uv run -q --with numpy --with scipy."""
exec(open(__import__('os').path.join(__import__('os').path.dirname(__file__),'p2d.py')).read().split('# Little')[0].split("d=collections.defaultdict(list)\nfor s in rows:")[0])
import numpy as np
from scipy.stats import spearmanr
X=[]
for s in rows:
    E=[x for x in ent[s]]; et=eat[s]
    byc=collections.defaultdict(list)
    for x in E: byc[route(x[4]%64)].append(x[0])
    cnt=lambda c,t,w=1000:B.bisect_right(byc[c],t)-B.bisect_left(byc[c],t-w)
    for x in rows[s]:
        if x[1]==23 and T0<=x[0]<=T1 and (s,x[3]) in raft_in and leader(x[0])==s and (s,x[2]) in skew:
            c,at=raft_in[(s,x[3])]; out=at-x[0]-skew[(s,x[2])]; g=pending_left(s,x[0])
            if g: continue
            X.append((out,open_at(s,x[0]),cnt('/read',x[0]),cnt('/commit',x[0])+cnt('/outcome',x[0])+cnt('/outcome_by_request',x[0]),cnt('/suspect',x[0]),x[4],x[5],s))
X=np.array(X,dtype=float); print('gate-open sends',len(X))
names=['open','reads/s','commit+outcome/s','suspect/s','awaiting fetches','loop lag']
for i,n in enumerate(names,1): print(f'  spearman(out, {n}) = {spearmanr(X[:,0],X[:,i])[0]:.2f}')
# out leg p50 by reads/s within open bands
print('out p50 (n) by leader edge /read arrivals in prior 1 s, within open band')
for lo,hi in ((0,100),(100,400),(400,10**9)):
    line=[]
    for a,b in ((0,50),(50,100),(100,200),(200,10**9)):
        m=(X[:,1]>=lo)&(X[:,1]<hi)&(X[:,2]>=a)&(X[:,2]<b)
        line.append(f'{a}-{b}: {int(np.median(X[m,0])) if m.sum() else None} ({m.sum()})')
    print(f'  open {lo}-{hi}:',' | '.join(line))
print('out p50 (n) by awaiting fetches within open band')
for lo,hi in ((0,100),(100,400),(400,10**9)):
    line=[]
    for a,b in ((0,6),(6,9),(9,16),(16,99)):
        m=(X[:,1]>=lo)&(X[:,1]<hi)&(X[:,5]>=a)&(X[:,5]<b)
        line.append(f'{a}-{b}: {int(np.median(X[m,0])) if m.sum() else None} ({m.sum()})')
    print(f'  open {lo}-{hi}:',' | '.join(line))
