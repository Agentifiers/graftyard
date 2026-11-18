#!/usr/bin/env python3
"""Point 2 of the voter-shed review (voter-shed-design.md, "Point 2"): m47 leader open requests by class, and the
leader->follower round trip split. Needs /tmp/shed/rows.pkl (prep.py). numpy/scipy: uv run -q --with numpy --with scipy."""
exec(open(__import__('os').path.join(__import__('os').path.dirname(__file__),'p2a.py')).read().split('for s in sorted(rows):')[0])
# Upper bound on the edge-read share of the leader's open count where open>=64: read residence <= the 90th pct of open/read-rate
# over low-commit, read-heavy bins (where nearly all open would be reads), applied as rate*bound over the >=64 bins.
for s in sorted(rows):
    E=[x for x in rows[s] if x[1]==25 and T0<=x[0]<=T1 and leader(x[0])==s]
    if len(E)<5000: continue
    b=collections.defaultdict(lambda:[0,0,[]])
    for x in E:
        r=route(x[4]%64); k=b[x[0]//1000]
        if r=='/read': k[0]+=1
        if r in ('/commit','/outcome','/outcome_by_request'): k[1]+=1
        k[2].append(x[5])
    m=lambda v:sum(v)/len(v)
    low=[m(v[2])/v[0] for v in b.values() if v[1]<10 and v[0]>=50]
    bound=pct(low,.9)
    hi=[v for v in b.values() if m(v[2])>=64]
    share=sum(v[0]*bound for v in hi)/sum(m(v[2]) for v in hi)
    print(f'member {s}: read residence bound p90 {bound*1000:.0f} ms over {len(low)} low-commit bins; edge reads <= {share:.1%} of open in {len(hi)} s with open>=64')
