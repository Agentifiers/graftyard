#!/usr/bin/env python3
"""(c) design evidence (tests/am1/results/c-design.md): m47 leader stalls, releases and the round trip. Needs
/tmp/shed/rows.pkl (prep.py); uv run -q --with numpy --with scipy."""
exec(open(__import__('os').path.join(__import__('os').path.dirname(__file__),'p2a.py')).read().split('for s in sorted(rows):')[0])
import bisect as B
for s in sorted(rows):
    V=rows[s]; at=[x[0] for x in V]
    sts=[(x[0]-x[5],x[0]) for x in V if x[1]==8 and T0<=x[0]<=T1 and leader(x[0])==s and x[5]>=300]
    if not sts: continue
    inside=collections.Counter(); dur=0; stamps=[]
    for a,b in sts:
        seg=V[B.bisect_right(at,a+5):B.bisect_left(at,b)]
        dur+=b-a
        for y in seg:
            k=route(y[4]%64)+':'+('edge' if y[2]==255 else 'peer') if y[1]==25 else {24:'CallLeft(/raft answered)',21:'AnswerArrived',23:'CallSent',4:'Applied',10:'Confirm'}.get(y[1])
            if k: inside[k]+=1
        stamps.append(len({y[0] for y in seg}))
    outside=collections.Counter()
    E=[y for y in V if T0<=y[0]<=T1 and leader(y[0])==s]
    tot=(max(y[0] for y in E)-min(y[0] for y in E))
    for y in E:
        k=route(y[4]%64)+':'+('edge' if y[2]==255 else 'peer') if y[1]==25 else {24:'CallLeft(/raft answered)',21:'AnswerArrived',23:'CallSent',4:'Applied',10:'Confirm'}.get(y[1])
        if k: outside[k]+=1
    print(f'member {s}: {len(sts)} stalls >=300 ms, {dur/1000:.1f} s; distinct clock stamps inside a stall p50 {pct(stamps,.5)}; per-second rates inside vs whole leading window:')
    for k,v in inside.most_common(8): print(f'    {k:28} {v/(dur/1000):7.0f}/s vs {outside[k]/(tot/1000):6.0f}/s')
