exec(open('tests/fanout/votershed/a3.py').read().split('tab=collections')[0])
import math
X=[]
for s in rows:
    for x in rows[s]:
        if x[1]==23 and T0<=x[0]<=T1 and (s,x[3]) in raft_in and leader(x[0])==s and (s,x[2]) in skew:
            callee,at,_=raft_in[(s,x[3])]
            o=at-x[0]-skew[(s,x[2])]
            op=open_at(s,x[0])
            i=bisect.bisect_left(eat[s],x[0]-1000); j=bisect.bisect_right(eat[s],x[0])
            rate=j-i
            edge_open=None
            X.append((o,op,rate))
def spearman(a,b):
    ra={v:i for i,v in enumerate(sorted(range(len(a)),key=lambda k:a[k]))}
    rb={v:i for i,v in enumerate(sorted(range(len(b)),key=lambda k:b[k]))}
    n=len(a); d=sum((ra[i]-rb[i])**2 for i in range(n)); return 1-6*d/(n*(n*n-1))
o=[x[0] for x in X]; print('n',len(X),'rho(out,open)',round(spearman(o,[x[1] for x in X]),3),'rho(out,rate1s)',round(spearman(o,[x[2] for x in X]),3))
# out leg by rate bins
rb=collections.defaultdict(list)
for oo,op,r in X: rb[min(r//100*100,1500)].append(oo)
print({k:(len(v),pct(v,.5),pct(v,.9)) for k,v in sorted(rb.items())})
# fraction of leader's sends made at open < B, and out-leg p90 for those
for B in [32,64,96,128,192,256]:
    lo=[oo for oo,op,r in X if op<B]; hi=[oo for oo,op,r in X if op>=B]
    print('B',B,'below: n',len(lo),'p50',pct(lo,.5),'p90',pct(lo,.9),'| at/above: n',len(hi),'p50',pct(hi,.5),'p90',pct(hi,.9))
