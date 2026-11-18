#!/usr/bin/env python3
"""outleg.py ROWS.pkl FROM TO TERMS -> leader-to-follower /raft out legs (pair skew corrected) by the leader's open
requests at send. ROWS.pkl as prep.py writes it, from any tsave.
  python3 tests/fanout/votershed/outleg.py /tmp/shed29/rows.pkl 2026-11-18T02:17:10Z 2026-11-18T02:27:10Z tests/fanout/raw/terms-m2bb29d.jsonl"""
import pickle,bisect,collections,datetime,json,sys
P=sys.argv[1]; T0s,T1s=sys.argv[2],sys.argv[3]; TERMS=sys.argv[4]
rows=pickle.load(open(P,'rb'))
iso=lambda s:int(datetime.datetime.fromisoformat(s.replace('Z','+00:00')).timestamp()*1000)
T0,T1=iso(T0s),iso(T1s)
pct=lambda xs,q:(sorted(xs)[min(len(xs)-1,int(q*len(xs)))] if xs else None)
ent={s:[x for x in v if x[1]==25] for s,v in rows.items()}; eat={s:[x[0] for x in e] for s,e in ent.items()}
def open_at(s,t):
    i=bisect.bisect_right(eat[s],t); return ent[s][i-1][5] if i else 0
raft_in={}
for s in ent:
    for x in ent[s]:
        if x[4]%64==1 and x[2]<128 and x[3]: raft_in[(x[2],x[3])]=(s,x[0])
left={(x[2],x[3]):x[0] for s in rows for x in rows[s] if x[1]==24}
ans={(s,x[3]):x[0] for s in rows for x in rows[s] if x[1]==21 and x[3]}
outs=collections.defaultdict(list); backs=collections.defaultdict(list)
for s in rows:
    for x in rows[s]:
        if x[1]==23 and (s,x[3]) in raft_in:
            c,at=raft_in[(s,x[3])]; outs[(s,c)].append(at-x[0])
            if (s,x[3]) in ans and (s,x[3]) in left: backs[(s,c)].append(ans[(s,x[3])]-left[(s,x[3])])
skew={p:(min(outs[p])-min(backs[p]))//2 for p in outs if backs[p]}
samples=[]
for l in open(TERMS):
    r=json.loads(l)
    if r.get('members'): samples.append((iso(r['at']),{m[0]:m[1] for m in r['members']}))
st=[t for t,_ in samples]
def leader(t):
    i=max(0,bisect.bisect_right(st,t)-1); return next((k for k,v in samples[i][1].items() if v=='leader'),None)
fb=[0,25,50,100,200,300,500,1000,10**9]
fbin=lambda o:max(b for b in fb if b<=o)
d=collections.defaultdict(list); n=0
for s in rows:
    for x in rows[s]:
        if x[1]==23 and T0<=x[0]<=T1 and (s,x[3]) in raft_in and leader(x[0])==s and (s,x[2]) in skew:
            c,at=raft_in[(s,x[3])]; d[fbin(open_at(s,x[0]))].append(at-x[0]-skew[(s,x[2])])
print('skews',skew)
print('leader open at send | n | out-leg p50 | p90')
for k in fb[:-1]:
    if k in d: print(f"{k:5} | {len(d[k]):5} | {pct(d[k],.5)} | {pct(d[k],.9)}")
L=[x[5] for s in ent for x in ent[s] if T0<=x[0]<=T1 and leader(x[0])==s]
print('leader open at entry p50/p90/p99/max',pct(L,.5),pct(L,.9),pct(L,.99),max(L) if L else None)
