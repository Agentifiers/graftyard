#!/usr/bin/env python3
# human-ahead (tests/am1/results/human-ahead.md): what capping the frozen batches ahead of a Human or MergeQueue command
# at K would have given on B0-33 m63r33. Usage: python3 tests/fanout/aheadreplay.py (reads the m63r33 leader trace).
# m63r33: for each cut that took a Human or MergeQueue command, its own entry's apply time (cut matched to its entry by
# the last appended index + frozen batches held + the entry in flight), and the same command moved K batches nearer.
import json, bisect, lzma
rows=set()
for l in lzma.open('tests/fanout/raw/tsave-m63r33.jsonl.xz', 'rt'):
    d=json.loads(l)
    if 'rows' not in d or 'learner' in d: continue
    for r in d['rows']: rows.add((d['site'],)+tuple(r))
t0=1795201456146
def pc(xs,q): xs=sorted(xs); return xs[min(len(xs)-1,int(len(xs)*q))]
for K in (2,1,0):
  before=[];after=[];moved=0
  for site in (0,1):
    app={}; apl={}
    for r in sorted(r for r in rows if r[0]==site and r[2]==12): app.setdefault(int(r[5]),r[1])
    for r in sorted(r for r in rows if r[0]==site and r[2]==4): apl.setdefault(int(r[5]),r[1])
    ai=sorted(app.items(), key=lambda x:(x[1],x[0])); times=[t for _,t in ai]; idx=[i for i,_ in ai]
    for r in rows:
      if r[0]!=site or r[2]!=29 or not t0<=r[1]<t0+300000: continue
      b=int(float(r[4]))
      if not (b&0xfff)+((b>>12)&0xfff): continue
      held=(r[3]//16)%4; k=bisect.bisect_right(times,r[1])
      if not k: continue
      e=max(idx[:k])+held+(1 if r[3]>=64 else 0)
      if e not in apl or apl[e]<r[1]: continue
      e2=e-max(0,held-K); moved+=e2!=e
      before.append(apl[e]-r[1]); after.append(apl.get(e2,apl[e])-r[1])
  print(f'at most {K} frozen ahead: {moved} of {len(before)} Human/MQ cuts move; cut to apply p50 {pc(before,.5)} -> {pc(after,.5)}, p90 {pc(before,.9)} -> {pc(after,.9)}, p99 {pc(before,.99)} -> {pc(after,.99)} ms')
