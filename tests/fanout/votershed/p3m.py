#!/usr/bin/env python3
"""(c) design evidence (tests/am1/results/c-design.md): m47 leader stalls, releases and the round trip. Needs
/tmp/shed/rows.pkl (prep.py); uv run -q --with numpy --with scipy."""
exec(open(__import__('os').path.join(__import__('os').path.dirname(__file__),'p2a.py')).read().split('for s in sorted(rows):')[0])
import bisect as B
# The clock freezes while JS runs; the first row stamped after a release shows how long its synchronous run held the
# isolate. Applied (kind 4) is logged in the release that resolves an entry's waiters.
for s in sorted(rows):
    V=rows[s]; at=[x[0] for x in V]
    com=sorted(x[0] for x in V if x[1]==25 and route(x[4]%64)=='/commit')
    R=[]; prev=None
    for i,x in enumerate(V):
        if x[1]!=4 or not(T0<=x[0]<=T1) or leader(x[0])!=s: continue
        j=B.bisect_right(at,x[0]); nxt=at[j]-x[0] if j<len(at) else None
        n=B.bisect_right(com,x[0])-(B.bisect_right(com,prev) if prev else B.bisect_left(com,x[0]-1000))
        prev=x[0]
        if nxt is not None: R.append((n,nxt))
    if len(R)<10: continue
    base=[at[k+1]-at[k] for k in range(len(at)-1) if T0<=at[k]<=T1 and at[k+1]>at[k]]
    print(f'member {s}: {len(R)} applies; gap to next stamped row after Applied p50/p90/max {pct([r[1] for r in R],.5)}/{pct([r[1] for r in R],.9)}/{max(r[1] for r in R)} ms; nonzero gaps between any rows p50/p90 {pct(base,.5)}/{pct(base,.9)}')
    for lo,hi in ((0,20),(20,150),(150,10**9)):
        v=[r[1] for r in R if lo<=r[0]<hi]; print(f'    /commit admitted since last apply {lo}-{hi}: n {len(v)} gap p50 {pct(v,.5)} p90 {pct(v,.9)}')
