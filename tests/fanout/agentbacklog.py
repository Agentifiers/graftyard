#!/usr/bin/env python3
"""agentbacklog.py TSAVE LOAD_START_MS -> the agent lane's wait from arrival to its cut, rebuilt from the leader's cut rows alone
(Cut 29: agent taken, 12 bits at 36, and left, 13 bits at 39): new arrivals between two cuts are taken+left less those
still queued, spread evenly over the gap, and taken FIFO; also how many cuts each passed over. An estimate: the trace
has no per-command id for the agent lane [inference]. tests/am1/results/learner-cadence.md, section 2."""
import json,lzma,sys,collections
path,t0=sys.argv[1],int(sys.argv[2]); t1=t0+300000
seen=set(); C=[]
for l in lzma.open(path,'rt'):
    d=json.loads(l)
    if 'rows' not in d or d.get('learner') is not None or d.get('site')!=0: continue
    for r in d['rows']:
        if r[1]!=29 or tuple(r) in seen: continue
        seen.add(tuple(r)); b=int(float(r[3])); ix=int(float(r[4]))
        C.append((r[0],(b>>36)&0xfff,(ix>>39)&0x1fff,int(r[2])%16))
C.sort()
# agent lane only (class index 3). FIFO by counts: arrivals between cuts spread evenly over the gap.
q=collections.deque(); waits=[]; cutsw=[]; prev=None; left_prev=0
for k,(at,taken,left,end) in enumerate(C):
    new=taken+left-len(q)
    if new>0:
        a0=prev if prev is not None else at
        for j in range(new): q.append((a0+(at-a0)*(j+1)/(new+1),k))
    for _ in range(min(taken,len(q))):
        a,k0=q.popleft()
        if t0<=a<t1: waits.append(at-a); cutsw.append(k-k0)
    while len(q)>left: q.pop()
    prev=at
pc=lambda v,p: sorted(v)[min(len(v)-1,int(p*len(v)))]
print(f' agent commands {len(waits)}: arrival->its cut p50/p90/p99 {pc(waits,.5):.0f}/{pc(waits,.9):.0f}/{pc(waits,.99):.0f} ms; capped cuts passed over',dict(sorted(collections.Counter(cutsw).items())))
