#!/usr/bin/env python3
"""followerload.py TSAVE LOAD_START_MS -> over a 300 s window with site 0 leading: each voter AppendEntries's queued->issued
and issued->ended (AppendIssued 13, AppendEnded 14), and each follower's open requests when a request entered (Entered 25's
`ms`) and when the leader's `/raft` entered (route 1, peer 0). tests/am1/results/learner-cadence.md, section 2."""
import json,lzma,sys,collections
path,t0=sys.argv[1],int(sys.argv[2]); t1=t0+300000
rows=collections.defaultdict(set)
for l in lzma.open(path,'rt'):
    d=json.loads(l)
    if 'rows' not in d or d.get('learner') is not None or d.get('site') is None: continue
    for r in d['rows']:
        if r[1] in (13,14,25) and t0<=r[0]<t1: rows[d['site']].add(tuple(r))
pc=lambda v,q: sorted(v)[min(len(v)-1,int(q*len(v)))] if v else None
L=rows[0]
for p in (1,2):
    iss=[r[5] for r in L if r[1]==13 and r[2]==p]; end=[r[5] for r in L if r[1]==14 and r[2]==p and r[4]==0]
    print(f'  leader->{p}: queued->issued p50/p90/p99 {pc(iss,.5)}/{pc(iss,.9)}/{pc(iss,.99)}; issued->ended ok p50/p90 {pc(end,.5)}/{pc(end,.9)}')
for s in (1,2):
    dep=[r[5] for r in rows[s] if r[1]==25]; ra=[r[5] for r in rows[s] if r[1]==25 and int(r[4])%64==1 and r[2]==0]
    print(f'  follower {s}: open requests at entry p50/p90 {pc(dep,.5)}/{pc(dep,.9)}; at the leader /raft entry p50/p90 {pc(ra,.5)}/{pc(ra,.9)}')
