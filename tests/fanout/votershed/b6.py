exec(open('tests/fanout/votershed/a3.py').read().split('tab=collections')[0])
# Leader sends to both followers within 20 ms of each other: compare their out legs (raw; per-pair skew constant)
S=collections.defaultdict(list)
for s in rows:
    for x in rows[s]:
        if x[1]==23 and T0<=x[0]<=T1 and (s,x[3]) in raft_in and leader(x[0])==s and (s,x[2]) in skew:
            callee,at,_=raft_in[(s,x[3])]
            S[s].append((x[0],x[2],at-x[0]-skew[(s,x[2])],x[4],x[5]))
pairs=[]
for L,v in S.items():
    v.sort()
    for i,a in enumerate(v):
        for b_ in v[i+1:]:
            if b_[0]-a[0]>20: break
            if b_[1]!=a[1]: pairs.append((a[2],b_[2])); break
import statistics
xs=[p[0] for p in pairs]; ys=[p[1] for p in pairs]
def spearman(a,b):
    ra={v:i for i,v in enumerate(sorted(range(len(a)),key=lambda k:a[k]))}
    rb={v:i for i,v in enumerate(sorted(range(len(b)),key=lambda k:b[k]))}
    n=len(a); return 1-6*sum((ra[i]-rb[i])**2 for i in range(n))/(n*(n*n-1))
print('pairs',len(pairs),'rho',round(spearman(xs,ys),3))
both=sum(1 for a,b_ in pairs if a>=1000 and b_>=1000); one=sum(1 for a,b_ in pairs if (a>=1000)!=(b_>=1000)); none_=sum(1 for a,b_ in pairs if a<1000 and b_<1000)
print('both>=1s',both,'one only',one,'neither',none_)
d=[abs(a-b_) for a,b_ in pairs if a>=1000 or b_>=1000]; print('|diff| where any>=1s p50',pct(d,.5),'p90',pct(d,.9))
