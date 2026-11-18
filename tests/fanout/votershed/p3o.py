#!/usr/bin/env python3
"""(c) design evidence (tests/am1/results/c-design.md): m47 leader stalls, releases and the round trip. Needs
/tmp/shed/rows.pkl (prep.py); uv run -q --with numpy --with scipy."""
exec(open(__import__('os').path.join(__import__('os').path.dirname(__file__),'p2d.py')).read().split('# Little')[0].split("d=collections.defaultdict(list)\nfor s in rows:")[0])
# Appends that carry the next entry, issued in the same release as the previous entry's Applied row (same ms stamp):
# skew-free round trip (less callee) by the /commit waiters that apply resolved (/commit entries since the previous apply).
RR=[]
for s in rows:
    V=rows[s]
    apms={}; prev=None
    com=sorted(x[0] for x in V if x[1]==25 and route(x[4]%64)=='/commit')
    for x in V:
        if x[1]==4:
            n=B.bisect_right(com,x[0])-(B.bisect_right(com,prev) if prev else B.bisect_left(com,x[0]-1000)); prev=x[0]; apms[x[0]]=n
    aq={(x[2],x[3]):x for x in V if x[1]==12}
    ai={}
    for x in V:
        if x[1]==13: ai[(x[2],x[0])]=x
    for x in V:
        if x[1]==23 and T0<=x[0]<=T1 and leader(x[0])==s and (s,x[3]) in raft_in and (s,x[3]) in ans and (s,x[3]) in left and (x[2],x[0]) in ai:
            c,at=raft_in[(s,x[3])]; rt=ans[(s,x[3])]-x[0]-(left[(s,x[3])]-at)
            same=apms.get(x[0])
            RR.append((rt, same, open_at(s,x[0])))
print('appends with entries, answered:',len(RR))
for lab,f in (('same ms as an Applied row',lambda r:r[1] is not None),('not',lambda r:r[1] is None)):
    v=[r for r in RR if f(r)]; print(f'  {lab}: n {len(v)} rt p50 {pct([r[0] for r in v],.5)} p90 {pct([r[0] for r in v],.9)}')
for lo,hi in ((0,20),(20,150),(150,10**9)):
    v=[r for r in RR if r[1] is not None and lo<=r[1]<hi]
    print(f'    resolved /commit {lo}-{hi}: n {len(v)} rt p50 {pct([r[0] for r in v],.5)} p90 {pct([r[0] for r in v],.9)} open p50 {pct([r[2] for r in v],.5)}')
