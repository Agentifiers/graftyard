#!/usr/bin/env python3
"""(c1b) review evidence (tests/am1/results/c-design.md, "(c1b) under review"): m47 leader stalls, tickets and poll load.
Needs /tmp/shed/rows.pkl (prep.py); uv run -q --with numpy --with scipy."""
exec(open(__import__('os').path.join(__import__('os').path.dirname(__file__),'p2a.py')).read().split('for s in sorted(rows):')[0])
import bisect as B
from statistics import median
def onset(V,at,t):
    i=B.bisect_right(at,t)
    if not i: return None
    t0=at[i-1]; return V[B.bisect_left(at,t0):i]
def feats(seg):
    c=collections.Counter(y[1] for y in seg)
    rd=sum(1 for y in seg if y[1]==25 and y[4]%64==2)
    return c[5],c[6],c[3],c[4],c[26],rd,len(seg)
S=[];Rnd=[]
import random; random.seed(3)
for s in sorted(rows):
    V=rows[s]; at=[x[0] for x in V]
    for x in V:
        if x[1]==8 and T0<=x[0]<=T1 and leader(x[0])==s and x[5]>=100:
            seg=onset(V,at,x[0]-x[5]); 
            if seg: S.append(feats(seg)+(x[5],))
    lead=[t for t in range(T0,T1,50) if leader(t)==s]
    for t in random.sample(lead,min(2000,len(lead))):
        seg=onset(V,at,t)
        if seg: Rnd.append(feats(seg))
names=['IndexTaken','ResSent','ResStepped','Applied','ReadQuery','/read entered','rows']
print('onset turn of stalls >=100 ms (n=%d) vs random leader times (n=%d): median [p90]'%(len(S),len(Rnd)))
for i,n in enumerate(names):
    a=sorted(r[i] for r in S); b=sorted(r[i] for r in Rnd)
    print(f'  {n:14} stall {median(a):6} [{a[int(.9*len(a))]}]   random {median(b):5} [{b[int(.9*len(b))]}]')
