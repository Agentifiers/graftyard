#!/usr/bin/env python3
"""shedshare.py ROWS.pkl FROM TO TERMS -> the share of edge /read and /outcome_by_request on the leader that entered
with the leader already at or past B open requests (B 32, 64, 128): what the voter shed would answer at once.
  python3 tests/fanout/votershed/shedshare.py /tmp/shed/rows.pkl 2026-11-18T01:00:18Z 2026-11-18T01:05:18Z tests/fanout/raw/terms-m47b33.jsonl"""
import pickle,bisect,collections,datetime,json,sys
P,T0s,T1s,TERMS=sys.argv[1:5]
rows=pickle.load(open(P,'rb'))
iso=lambda s:int(datetime.datetime.fromisoformat(s.replace('Z','+00:00')).timestamp()*1000)
T0,T1=iso(T0s),iso(T1s)
R=["/raft","/read","/commit","/commit_many","/outcome","/outcome_by_request","/suspect"]
samples=[]
for l in open(TERMS):
    r=json.loads(l)
    if r.get('members'): samples.append((iso(r['at']),{m[0]:m[1] for m in r['members']}))
st=[t for t,_ in samples]
def leader(t):
    i=max(0,bisect.bisect_right(st,t)-1); return next((k for k,v in samples[i][1].items() if v=='leader'),None)
for B in (32,64,128):
    tot=collections.Counter(); sh=collections.Counter()
    for s,v in rows.items():
        for x in v:
            if x[1]==25 and T0<=x[0]<=T1 and x[2]==255 and leader(x[0])==s:
                c=x[4]%64; r=R[c-1] if 0<c<=7 else 'o'
                if r in ('/read','/outcome_by_request'):
                    tot[r]+=1
                    if x[5]-1>=B: sh[r]+=1
    print('B',B,{r:f"{sh[r]}/{tot[r]} = {sh[r]/max(1,tot[r]):.3f}" for r in tot})
