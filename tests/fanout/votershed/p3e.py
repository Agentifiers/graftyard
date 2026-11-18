#!/usr/bin/env python3
"""(c) design evidence (tests/am1/results/c-design.md): m47 leader stalls, releases and the round trip. Needs
/tmp/shed/rows.pkl (prep.py); uv run -q --with numpy --with scipy."""
exec(open(__import__('os').path.join(__import__('os').path.dirname(__file__),'p2d.py')).read().split('# Little')[0].split("d=collections.defaultdict(list)\nfor s in rows:")[0])
# Ticker lag rows (kind 8: the 100 ms ticker woke `ms` late, stamped at the wake) mark a stretch with no turn for the
# ticker: [at-ms, at]. Does a call's round trip overlap such a stretch, and by how much?
R=[]
for s in rows:
    L=sorted((x[0]-x[5],x[0]) for x in rows[s] if x[1]==8)
    for x in rows[s]:
        if x[1]==23 and T0<=x[0]<=T1 and (s,x[3]) in raft_in and leader(x[0])==s and (s,x[3]) in ans and (s,x[3]) in left:
            c,at=raft_in[(s,x[3])]; a,b=x[0],ans[(s,x[3])]
            rt=b-a-(left[(s,x[3])]-at)
            ov=sum(max(0,min(b,e)-max(a,st)) for st,e in L if st<b and e>a)
            R.append((rt,ov,open_at(s,x[0])))
print('answered calls',len(R))
for lo,hi in ((0,64),(64,300),(300,10**9),(0,10**9)):
    S=[r for r in R if lo<=r[2]<hi]; ex=sum(max(0,r[0]-300) for r in S); cov=sum(min(r[1],max(0,r[0]-300)) for r in S)
    w=[r for r in S if r[1]>0]
    print(f' open {lo}-{hi}: n {len(S)}, overlapping a ticker stall {len(w)} ({len(w)/max(1,len(S)):.0%}); share of round-trip excess over 300 ms covered by ticker stalls {cov/max(1,ex):.0%}; rt p50 with stall {pct([r[0] for r in w],.5)} without {pct([r[0] for r in S if r[1]==0],.5)}')
for s in rows:
    L=[x[5] for x in rows[s] if x[1]==8 and T0<=x[0]<=T1 and leader(x[0])==s]
    if L: print(f' member {s} as leader: ticker stalls >=20 ms {len(L)}, total {sum(L)/1000:.1f} s, p50 {pct(L,.5)} max {max(L)}')
