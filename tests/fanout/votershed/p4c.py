#!/usr/bin/env python3
"""(c1b) review evidence (tests/am1/results/c-design.md, "(c1b) under review"): m47 leader stalls, tickets and poll load.
Needs /tmp/shed/rows.pkl (prep.py); uv run -q --with numpy --with scipy."""
exec(open(__import__('os').path.join(__import__('os').path.dirname(__file__),'p2a.py')).read().split('for s in sorted(rows):')[0])
import bisect as B
# A stall's work: every row stamped from the last clock value before the stall's start through its end. The clock only
# moves on I/O, so the rows the isolate wrote while the ticker could not run carry these few stamps.
comp=collections.Counter(); base=collections.Counter(); st_ms=0; lead_ms=0; with_apply=0; n=0; per=[]
for s in sorted(rows):
    V=rows[s]; at=[x[0] for x in V]
    for x in V:
        if x[1]==8 and T0<=x[0]<=T1 and leader(x[0])==s and x[5]>=100:
            a,b=x[0]-x[5],x[0]; i=B.bisect_right(at,a)
            if not i: continue
            seg=V[B.bisect_left(at,at[i-1]):B.bisect_left(at,b)]
            c=collections.Counter((route(y[4]%64) if y[1]==25 else None) for y in seg); c.pop(None,None)
            comp+=c; st_ms+=x[5]; n+=1; with_apply+=any(y[1]==4 for y in seg)
            per.append((x[5],sum(c.values()),c['/read'],c['/commit'],c['/outcome_by_request'],c['/suspect'],c['/raft']))
    E=[y for y in V if y[1]==25 and T0<=y[0]<=T1 and leader(y[0])==s]
    base+=collections.Counter(route(y[4]%64) for y in E)
tc=sum(comp.values()); tb=sum(base.values())
print(f'{n} leader stalls >=100 ms, {st_ms/1000:.1f} s; {with_apply} ({with_apply/n:.0%}) contain an Applied row; requests entered during them {tc} ({tc/(st_ms/1000):.0f}/s)')
print('composition of requests entered during stalls vs all leader entries:')
for k in ('/read','/outcome_by_request','/commit','/suspect','/raft','/outcome','other'):
    print(f'   {k:20} {comp[k]/tc:5.0%} vs {base[k]/tb:5.0%}')
from scipy.stats import spearmanr
per=sorted(per)
print('spearman(stall ms, requests entered in it)',round(spearmanr([p[0] for p in per],[p[1] for p in per])[0],2))
for i,k in enumerate(['/read','/commit','/outcome_by_request','/suspect','/raft'],2):
    print(f'   spearman(stall ms, {k} in it) {spearmanr([p[0] for p in per],[p[i] for p in per])[0]:.2f}')
print('requests per stall second p50',sorted(p[1]/(p[0]/1000) for p in per)[len(per)//2])
