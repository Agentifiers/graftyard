#!/usr/bin/env python3
"""Learner /raft calls on the leader per second, with read-index and held-append rates (m50). Edit p2a.py for the run."""
exec(open(__import__('os').path.join(__import__('os').path.dirname(__file__),'p2a.py')).read().split('for s in sorted(rows):')[0])
import bisect as B
for s in sorted(rows):
    V=[x for x in rows[s] if T0<=x[0]<=T1 and leader(x[0])==s]
    E=[x for x in V if x[1]==25 and x[4]%64==1 and x[2]>=192]
    if len(E)<1000: continue
    secs=len({x[0]//1000 for x in V if x[1]==25})
    taken=[x for x in V if x[1]==5 and x[2]>=192]      # IndexTaken from a learner
    held=[x for x in V if x[1]==19 and x[2]>=192]      # AppendTaken: a held append left in a learner's call
    resp=[x for x in V if x[1]==6 and x[2]>=192]
    per=collections.Counter(x[2] for x in E)
    print(f'member {s}: learner /raft calls {len(E)/secs:.1f}/s over {secs} s ({ {k:round(v/secs,1) for k,v in sorted(per.items())} })')
    print(f'   ReadIndex taken from learners {len(taken)/secs:.1f}/s; held appends taken {len(held)/secs:.1f}/s; ResSent to learners {len(resp)/secs:.1f}/s')
