#!/usr/bin/env python3
"""Point 2 of the voter-shed review (voter-shed-design.md, "Point 2"): m47 leader open requests by class, and the
leader->follower round trip split. Needs /tmp/shed/rows.pkl (prep.py). numpy/scipy: uv run -q --with numpy --with scipy."""
exec(open(__import__('os').path.join(__import__('os').path.dirname(__file__),'p2a.py')).read().split('for s in sorted(rows):')[0])
import bisect as B
COMMITF=('/commit','/outcome','/outcome_by_request')
ent={s:[x for x in rows[s] if x[1]==25] for s in rows}; eat={s:[x[0] for x in e] for s,e in ent.items()}
def oa(s,t):
    i=B.bisect_right(eat[s],t); return ent[s][i-1][5] if i else 0
byc={s:collections.defaultdict(list) for s in rows}
for s in rows:
    for x in ent[s]: byc[s][route(x[4]%64)].append(x[0])
cnt=lambda s,cs,t,w:sum(B.bisect_right(byc[s][c],t)-B.bisect_left(byc[s][c],t-w) for c in cs)
print('1 s bins while leading: median leader open, by commit-family (/commit,/outcome,/outcome_by_request) arrivals in the prior 3 s')
for s in sorted(rows):
    bins=collections.defaultdict(list)
    for x in ent[s]:
        if T0<=x[0]<=T1 and leader(x[0])==s: bins[x[0]//1000].append(x)
    if len(bins)<20: continue
    out=collections.defaultdict(lambda:[[],[]])
    for b,xs in bins.items():
        c=cnt(s,COMMITF,b*1000+999,3000); r=sum(route(x[4]%64)=='/read' for x in xs)
        k=0 if c==0 else 1 if c<30 else 2 if c<150 else 3
        out[k][0].append(sorted(x[5] for x in xs)[len(xs)//2]); out[k][1].append(r)
    for k,lab in enumerate(('0','1-29','30-149','150+')):
        if out[k][0]: print(f'  member {s} commit-family {lab:>6}: bins {len(out[k][0]):3} open p50 {pct(out[k][0],.5):5} p90 {pct(out[k][0],.9):5}  edge reads/s p50 {pct(out[k][1],.5):4} max {max(out[k][1])}')
