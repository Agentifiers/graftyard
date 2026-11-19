#!/usr/bin/env python3
"""rttband.py TSAVE FROM TO TERMS -> the leader's /raft call round trips (CallSent 23 to AnswerArrived 21, one
clock) by the follower's inbound requests and whole-tier dumps in that second (B0-33 m53's F91 check)."""
import json,collections,bisect,datetime,lzma,sys
P,F,T,TERMS=sys.argv[1:5]
iso=lambda s:int(datetime.datetime.fromisoformat(s.replace("Z","+00:00")).timestamp()*1000)
T0,T1=iso(F),iso(T)
rows=collections.defaultdict(set)
for line in (lzma.open if P.endswith('.xz') else open)(P,'rt'):
    d=json.loads(line)
    if 'rows' in d and 'learner' not in d: rows[d['site']].update(tuple(x) for x in d['rows'] if x[1] in (21,23,25,26))
S=[(iso(d['at']),{m[0]:m[1] for m in d['members']}) for d in map(json.loads,open(TERMS)) if d.get('members')]
st=[t for t,_ in S]
role=lambda t,s:S[max(0,bisect.bisect_right(st,t)-1)][1].get(s)
pc=lambda v,p:sorted(v)[min(len(v)-1,int(p*len(v)))] if v else None
load=collections.Counter(); tier=collections.Counter()
for s,V in rows.items():
    q={x[3]:x[4] for x in V if x[1]==26}
    for x in V:
        if x[1]==25:
            load[(s,x[0]//1000)]+=1
            if x[4]%64==2 and q.get(x[4]//64)==22: tier[(s,x[0]//1000)]+=1
A=collections.defaultdict(list); Bt=collections.defaultdict(list)
for L in rows:
    ans={x[3]:x[0] for x in rows[L] if x[1]==21}
    for x in rows[L]:
        if x[1]!=23 or x[2] not in (0,1,2) or not T0<=x[0]<T1 or role(x[0],L)!='leader': continue
        a=ans.get(x[3]); r=(a-x[0]) if a else 99999
        sec=(x[2],x[0]//1000)
        l=max(load[sec],load[(x[2],sec[1]+1)])
        b='0-49' if l<50 else '50-149' if l<150 else '150-399' if l<400 else '400+'
        A[b].append(r)
        n=tier[sec]+tier[(x[2],sec[1]+1)]
        Bt['0' if n==0 else '1-4' if n<5 else '5+'].append(r)
for name,D,keys in (('follower inbound /s',A,('0-49','50-149','150-399','400+')),('follower whole-tier dumps entered (2 s)',Bt,('0','1-4','5+'))):
    for b in keys:
        v=D.get(b,[]); print(f"  {name} {b:8}: calls {len(v):4}  rtt p50 {pc(v,.5)} p90 {pc(v,.9)}  >=1s or unanswered {sum(o>=1000 for o in v)} ({100*sum(o>=1000 for o in v)/max(1,len(v)):.0f}%)")
