#!/usr/bin/env python3
"""(c1b) review evidence (tests/am1/results/c-design.md, "(c1b) under review"): m47 leader stalls, tickets and poll load.
Needs /tmp/shed/rows.pkl (prep.py); uv run -q --with numpy --with scipy."""
exec(open(__import__('os').path.join(__import__('os').path.dirname(__file__),'p2a.py')).read().split('for s in sorted(rows):')[0])
import lzma, bisect as B
# (1) open tickets on the leader during load (memguard inFlight.admitted), and the cap's added poll load
T=[]
for l in lzma.open('/home/user/workspace/repo/tests/fanout/raw/memguard-m47b33.jsonl.xz','rt'):
    r=json.loads(l); t=iso(r['at'])
    if T0<=t<=T1:
        for m in r['members']:
            if m['role'] and m['memSplit']: T.append(m['memSplit']['inFlight']['admitted'])
T.sort(); q=lambda p:T[min(len(T)-1,int(p*len(T)))]
print(f'leader open tickets, {len(T)} samples: p50 {q(.5)} p90 {q(.9)} max {T[-1]}')
for C in (128,256,512):
    row=[]
    for W,lab in ((576,'B0-33 sixth pass max 576'),(q(.5),'m47 p50'),(q(.9),'m47 p90'),(1888,'m47 max waiters/apply 1,888')):
        ex=max(0,W-C); row.append(f'{lab}: {ex/0.25:.0f} | {ex/1:.0f} | {ex/2:.0f}')
    print(f' cap {C}: added polls/s at retry 250 ms | 1 s | 2 s ->',' ; '.join(row))
# (2) control for the Applied onset: stalls vs random frozen-stamp bursts of >=100 rows
st=[];rb=[]
import random; random.seed(4)
for s in sorted(rows):
    V=rows[s]; at=[x[0] for x in V]
    stamps=collections.defaultdict(list)
    for x in V:
        if T0<=x[0]<=T1: stamps[x[0]].append(x[1])
    big=[t for t,k in stamps.items() if len(k)>=100 and leader(t)==s]
    for x in V:
        if x[1]==8 and T0<=x[0]<=T1 and leader(x[0])==s and x[5]>=100:
            i=B.bisect_right(at,x[0]-x[5])
            if i: st.append(4 in stamps.get(at[i-1],[]))
    rb+= [4 in stamps[t] for t in big]
print(f'onset burst holds an Applied row: stalls {sum(st)}/{len(st)} ({sum(st)/len(st):.0%}); all frozen-stamp bursts of >=100 rows {sum(rb)}/{len(rb)} ({sum(rb)/max(1,len(rb)):.0%})')
