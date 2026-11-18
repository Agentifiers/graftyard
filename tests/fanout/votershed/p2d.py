#!/usr/bin/env python3
"""Point 2 of the voter-shed review (voter-shed-design.md, "Point 2"): m47 leader open requests by class, and the
leader->follower round trip split. Needs /tmp/shed/rows.pkl (prep.py). numpy/scipy: uv run -q --with numpy --with scipy."""
exec(open(__import__('os').path.join(__import__('os').path.dirname(__file__),'p2a.py')).read().split('for s in sorted(rows):')[0])
import bisect as B, numpy as np
ent={s:[x for x in rows[s] if x[1]==25] for s in rows}; eat={s:[x[0] for x in e] for s,e in ent.items()}
def open_at(s,t):
    i=B.bisect_right(eat[s],t); return ent[s][i-1][5] if i else 0
raft_in={}
for s in ent:
    for x in ent[s]:
        if x[4]%64==1 and x[2]<128 and x[3]: raft_in[(x[2],x[3])]=(s,x[0])
left={(x[2],x[3]):x[0] for s in rows for x in rows[s] if x[1]==24}
ans={(s,x[3]):x[0] for s in rows for x in rows[s] if x[1]==21 and x[3]}
outs=collections.defaultdict(list); backs=collections.defaultdict(list)
for s in rows:
    for x in rows[s]:
        if x[1]==23 and (s,x[3]) in raft_in:
            c,at=raft_in[(s,x[3])]; outs[(s,c)].append(at-x[0])
            if (s,x[3]) in ans and (s,x[3]) in left: backs[(s,c)].append(ans[(s,x[3])]-left[(s,x[3])])
skew={p:(min(outs[p])-min(backs[p]))//2 for p in outs if backs[p]}
# confirm-pending intervals per member (Confirm row: at = resolution, ms = sync duration); flush deferred intervals
pend={s:sorted((x[0]-x[5],x[0]) for x in rows[s] if x[1]==10) for s in rows}
def pending_left(s,t):
    # time from t until every confirm started at or before t resolved (0 if gate open)
    m=0
    for a,b in pend[s]:
        if a>t: break
        if b>t: m=max(m,b-t)
    return m
d=collections.defaultdict(list)
for s in rows:
    for x in rows[s]:
        if x[1]==23 and T0<=x[0]<=T1 and (s,x[3]) in raft_in and leader(x[0])==s and (s,x[2]) in skew:
            c,at=raft_in[(s,x[3])]; out=at-x[0]-skew[(s,x[2])]; g=pending_left(s,x[0]); o=open_at(s,x[0])
            d[(g>0, min(o//100*100,1000) if o>=100 else (0 if o<25 else 25 if o<64 else 64))].append((out,max(0,out-g),g))
print('out leg by (leader gate closed at send, leader open at send): n, out p50/p90, out beyond the gate p50/p90, gate left p50')
for k in sorted(d):
    v=d[k]; print(f'  gate_closed={k[0]!s:5} open>={k[1]:4} n {len(v):5}  out {pct([a for a,b,c in v],.5):5} {pct([a for a,b,c in v],.9):5}  beyond {pct([b for a,b,c in v],.5):5} {pct([b for a,b,c in v],.9):5}  gate {pct([c for a,b,c in v],.5)}')
# Little's law split of the leader's open count by class, 1 s bins while leading
C=['/read','/outcome_by_request','/commit','/suspect','/raft','/outcome']
for s in sorted(rows):
    E=[x for x in ent[s] if T0<=x[0]<=T1 and leader(x[0])==s]
    if len(E)<5000: continue
    sec=collections.defaultdict(collections.Counter); op=collections.defaultdict(list)
    for x in E:
        b=x[0]//1000; sec[b][route(x[4]%64)]+=1; op[b].append(x[5])
    bins=sorted(op)
    for W in (3,6,10):
        A=np.array([[sum(sec[b-k][c] for k in range(W))/W for c in C] for b in bins]); y=np.array([np.mean(op[b]) for b in bins])
        from scipy.optimize import nnls
        D,_=nnls(A,y); fit=A@D; r2=1-((y-fit)**2).sum()/((y-y.mean())**2).sum()
        hi=y>=64
        sh=(A[hi]*D).sum(0)/(A[hi]*D).sum()
        print(f'member {s} W={W}s bins {len(bins)} R2 {r2:.2f} residence s',{c:round(v,2) for c,v in zip(C,D)},'share of open in bins with open>=64',{c:round(v,2) for c,v in zip(C,sh)})
