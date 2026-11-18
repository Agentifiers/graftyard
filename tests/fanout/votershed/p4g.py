#!/usr/bin/env python3
"""(c1b) review evidence (tests/am1/results/c-design.md, "(c1b) under review"): m47 leader stalls, tickets and poll load.
Needs /tmp/shed/rows.pkl (prep.py); uv run -q --with numpy --with scipy."""
exec(open(__import__('os').path.join(__import__('os').path.dirname(__file__),'p2a.py')).read().split('for s in sorted(rows):')[0])
import bisect as B
# Upper bound on the release's share of stall time: for each leader Applied row, the gap to the next stamped row (the
# release turn and what queued behind it), clipped to ticker stalls.
tot_st=0; tot_rel=0; cnt=collections.Counter()
for s in sorted(rows):
    V=rows[s]; at=[x[0] for x in V]
    L=[(x[0]-x[5],x[0]) for x in V if x[1]==8 and T0<=x[0]<=T1 and leader(x[0])==s and x[5]>=100]
    tot_st+=sum(b-a for a,b in L)
    for x in V:
        if x[1]==4 and T0<=x[0]<=T1 and leader(x[0])==s:
            j=B.bisect_right(at,x[0]); 
            if j>=len(at): continue
            a,b=x[0],at[j]
            ov=sum(max(0,min(b,e)-max(a,st)) for st,e in L)
            tot_rel+=ov; cnt['applies']+=1; cnt['gap>=100']+= (b-a)>=100
print(f'leader stall time {tot_st/1000:.1f} s; of it inside the gap right after an Applied row {tot_rel/1000:.1f} s ({tot_rel/tot_st:.0%}); applies {cnt["applies"]}, with a gap of 100 ms or more {cnt["gap>=100"]}')
