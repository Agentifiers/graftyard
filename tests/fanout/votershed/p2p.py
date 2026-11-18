#!/usr/bin/env python3
"""Point 2 of the voter-shed review (voter-shed-design.md, "Point 2"): m47 leader open requests by class, and the
leader->follower round trip split. Needs /tmp/shed/rows.pkl (prep.py). numpy/scipy: uv run -q --with numpy --with scipy."""
exec(open(__import__('os').path.join(__import__('os').path.dirname(__file__),'p2d.py')).read().split('# Little')[0].split("d=collections.defaultdict(list)\nfor s in rows:")[0])
R=[]
for s in rows:
    for x in rows[s]:
        if x[1]==23 and T0<=x[0]<=T1 and (s,x[3]) in raft_in and leader(x[0])==s and (s,x[3]) in ans and (s,x[3]) in left:
            c,at=raft_in[(s,x[3])]; rt=ans[(s,x[3])]-x[0]-(left[(s,x[3])]-at)
            R.append((rt,pending_left(s,x[0]),x[5],open_at(s,x[0])))
for lo,hi,lab in ((0,64,'<64'),(64,300,'64-299'),(300,10**9,'300+'),(0,10**9,'all')):
    S=[r for r in R if lo<=r[3]<hi]; ex=g=l=0
    for rt,gate,lag,o in S:
        e=max(0,rt-300); a=min(gate,e); b=min(lag,e-a); ex+=e; g+=a; l+=b
    print(f'open {lab:6} n {len(S):4} round trip less callee p50 {pct([r[0] for r in S],.5)} p90 {pct([r[0] for r in S],.9)}; excess over 300 ms: gate left at send {g/max(1,ex):.0%}, then loop lag at send {l/max(1,ex):.0%}, rest (wire, the answer waiting at the leader) {1-(g+l)/max(1,ex):.0%}')
