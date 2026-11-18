#!/usr/bin/env python3
"""(c) design evidence (tests/am1/results/c-design.md): m47 leader stalls, releases and the round trip. Needs
/tmp/shed/rows.pkl (prep.py); uv run -q --with numpy --with scipy."""
exec(open(__import__('os').path.join(__import__('os').path.dirname(__file__),'p2d.py')).read().split('# Little')[0].split("d=collections.defaultdict(list)\nfor s in rows:")[0])
import pickle
# (a) apply lag: leader Applied(idx) time vs each follower's Applied(idx), skew-corrected (follower clock -> leader clock)
app={s:{} for s in rows}
for s in rows:
    for x in rows[s]:
        if x[1]==4 and x[4] not in app[s]: app[s][x[4]]=x[0]
lag=collections.defaultdict(list)
for L in rows:
    for idx,t in app[L].items():
        if not(T0<=t<=T1) or leader(t)!=L: continue
        for F in rows:
            if F!=L and idx in app[F] and (L,F) in skew:
                lag[F].append(app[F][idx]-skew[(L,F)]-t)
for F in sorted(lag): print(f'follower {F}: applied an index after the leader did, ms p10/p50/p90/p99 {pct(lag[F],.1)}/{pct(lag[F],.5)}/{pct(lag[F],.9)}/{pct(lag[F],.99)} n {len(lag[F])}')
# (b) round trip by entries the leader applied in the 500 ms before the send, within open band
R=[]
for s in rows:
    at=sorted((t,i) for i,t in app[s].items()); att=[a for a,_ in at]
    for x in rows[s]:
        if x[1]==23 and T0<=x[0]<=T1 and (s,x[3]) in raft_in and leader(x[0])==s and (s,x[3]) in ans and (s,x[3]) in left:
            c,a=raft_in[(s,x[3])]; rt=ans[(s,x[3])]-x[0]-(left[(s,x[3])]-a)
            n=B.bisect_right(att,x[0])-B.bisect_left(att,x[0]-500)
            R.append((rt,n,open_at(s,x[0])))
print('round trip less callee p50/p90 (n): sends with vs without a leader apply in the prior 500 ms')
for lo,hi in ((0,64),(64,300),(300,10**9)):
    a=[r[0] for r in R if lo<=r[2]<hi and r[1]>0]; b=[r[0] for r in R if lo<=r[2]<hi and r[1]==0]
    print(f'  open {lo}-{hi}: after an apply {pct(a,.5)}/{pct(a,.9)} ({len(a)}) | none {pct(b,.5)}/{pct(b,.9)} ({len(b)})')
