#!/usr/bin/env python3
"""Point 2 of the voter-shed review (voter-shed-design.md, "Point 2"): m47 leader open requests by class, and the
leader->follower round trip split. Needs /tmp/shed/rows.pkl (prep.py). numpy/scipy: uv run -q --with numpy --with scipy."""
exec(open(__import__('os').path.join(__import__('os').path.dirname(__file__),'p2d.py')).read().split('# Little')[0].split("d=collections.defaultdict(list)\nfor s in rows:")[0])
import numpy as np
from scipy.stats import spearmanr
S=[]  # (s, send t, out, open, since last write, loop lag)
for s in rows:
    W=sorted(x[0] for x in rows[s] if x[1] in (15,16))
    for x in rows[s]:
        if x[1]==23 and T0<=x[0]<=T1 and (s,x[3]) in raft_in and leader(x[0])==s and (s,x[2]) in skew:
            c,at=raft_in[(s,x[3])]; out=at-x[0]-skew[(s,x[2])]
            i=B.bisect_right(W,x[0]); since=x[0]-W[i-1] if i else 10**6
            S.append((s,x[0],out,open_at(s,x[0]),since,x[5]))
print('sends',len(S))
print('out leg p50/p90 (n) by time since the leader last wrote, within open band')
for lo,hi in ((0,100),(100,400),(400,10**9)):
    line=[]
    for a,b in ((0,50),(50,250),(250,1000),(1000,10**9)):
        v=[r[2] for r in S if lo<=r[3]<hi and a<=r[4]<b]
        line.append(f'{a}-{b}ms: {pct(v,.5)}/{pct(v,.9)} ({len(v)})')
    print(f'  open {lo}-{hi}:',' | '.join(line))
print('out leg p50 (n) by loop lag at send, within open band')
for lo,hi in ((0,100),(100,400),(400,10**9)):
    line=[]
    for a,b in ((0,1),(1,100),(100,500),(500,10**9)):
        v=[r[2] for r in S if lo<=r[3]<hi and a<=r[5]<b]
        line.append(f'{a}-{b}: {pct(v,.5)} ({len(v)})')
    print(f'  open {lo}-{hi}:',' | '.join(line))
# lead/lag: 1 s bins per leader member, mean open at entries vs mean out leg of sends
for s in sorted(rows):
    o=collections.defaultdict(list); q=collections.defaultdict(list)
    for x in ent[s]:
        if T0<=x[0]<=T1 and leader(x[0])==s: o[x[0]//1000].append(x[5])
    for r in S:
        if r[0]==s: q[r[1]//1000].append(r[2])
    bins=[b for b in sorted(o) if b in q]
    if len(bins)<20: continue
    print(f'member {s}: spearman(open at t+k, out leg at t) over {len(bins)} 1 s bins; k<0 means open earlier')
    line=[]
    for k in (-5,-3,-2,-1,0,1,2,3,5):
        pairs=[(np.mean(o[b+k]),np.median(q[b])) for b in bins if (b+k) in o]
        line.append(f'k={k}: {spearmanr([p[0] for p in pairs],[p[1] for p in pairs])[0]:.2f}')
    print('   ',' '.join(line))
