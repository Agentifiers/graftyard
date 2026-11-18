#!/usr/bin/env python3
"""(c1b) review evidence (tests/am1/results/c-design.md, "(c1b) under review"): m47 leader stalls, tickets and poll load.
Needs /tmp/shed/rows.pkl (prep.py); uv run -q --with numpy --with scipy."""
exec(open(__import__('os').path.join(__import__('os').path.dirname(__file__),'p2a.py')).read().split('for s in sorted(rows):')[0])
import bisect as B
K={1:'IndexSent',2:'IndexEnded',3:'ResStepped',4:'Applied',5:'IndexTaken',6:'ResSent',7:'ResEnded',10:'Confirm',12:'AQ',13:'AI',14:'AE',15:'Stepped',16:'Flushed',17:'AStepped',18:'AHeld',19:'ATaken',20:'CallArrived',21:'AnsArr',23:'CallSent',24:'CallLeft',26:'ReadQuery',27:'SentBy',28:'Reported'}
tot=collections.Counter(); totms=collections.Counter()
for s in sorted(rows):
    V=rows[s]; at=[x[0] for x in V]
    sts=[(x[0]-x[5],x[5]) for x in V if x[1]==8 and T0<=x[0]<=T1 and leader(x[0])==s and x[5]>=100]
    for t,ms in sts:
        # rows stamped at the last clock value at or before the stall start: the turn that began it
        i=B.bisect_right(at,t)
        if not i: continue
        t0=at[i-1]; seg=V[B.bisect_left(at,t0):i]
        kinds=collections.Counter(K.get(y[1],route(y[4]%64) if y[1]==25 else y[1]) for y in seg)
        top='Applied' if kinds.get('Applied') else 'ResSent/IndexTaken (read grants)' if (kinds.get('ResSent') or kinds.get('IndexTaken')) else 'Confirm' if kinds.get('Confirm') else kinds.most_common(1)[0][0]
        tot[top]+=1; totms[top]+=ms
print('stalls >=100 ms on leaders, by what the last stamped clock value before the stall carried (priority Applied > read grants > Confirm > most common row):')
for k,v in totms.most_common(): print(f'  {k:34} stalls {tot[k]:3}  stall time {v/1000:5.1f} s ({v/sum(totms.values()):.0%})')
import random
random.seed(2)
base=collections.Counter(); n=0
for s in sorted(rows):
    V=rows[s]; at=[x[0] for x in V]
    lead=[t for t in range(T0,T1,50) if leader(t)==s]
    for t in random.sample(lead,min(3000,len(lead))):
        i=B.bisect_right(at,t)
        if not i: continue
        t0=at[i-1]; seg=V[B.bisect_left(at,t0):i]
        kinds=collections.Counter(K.get(y[1],route(y[4]%64) if y[1]==25 else y[1]) for y in seg)
        top='Applied' if kinds.get('Applied') else 'ResSent/IndexTaken (read grants)' if (kinds.get('ResSent') or kinds.get('IndexTaken')) else 'Confirm' if kinds.get('Confirm') else kinds.most_common(1)[0][0]
        base[top]+=1; n+=1
print('base rate at random leader times:',{k:f'{v/n:.0%}' for k,v in base.most_common(6)})
