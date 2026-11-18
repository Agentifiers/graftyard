import pickle,bisect,collections,datetime
rows=pickle.load(open('/tmp/shed/rows.pkl','rb'))
iso=lambda s:int(datetime.datetime.fromisoformat(s.replace('Z','+00:00')).timestamp()*1000)
T0,T1=iso('2026-11-18T01:00:18Z'),iso('2026-11-18T01:05:18Z')
def pct(xs,q):
    xs=sorted(xs); return xs[min(len(xs)-1,int(q*len(xs)))] if xs else None
ROUTES=["/raft","/read","/commit","/commit_many","/outcome","/outcome_by_request","/suspect","/campaign","/status","/stats","/trace","/ping","/rtt","/place","/learners","/record"]
route=lambda c: ROUTES[c-1] if 0<c<=len(ROUTES) else 'other'
ent={s:[x for x in v if x[1]==25] for s,v in rows.items()}
eat={s:[x[0] for x in e] for s,e in ent.items()}
def open_at(s,t):
    i=bisect.bisect_right(eat[s],t)
    return ent[s][i-1][5] if i else 0
bins=[0,50,100,200,300,500,750,1000,1500,10**9]
def b(o):
    for i in range(len(bins)-1):
        if bins[i]<=o<bins[i+1]: return f"{bins[i]}-{bins[i+1] if bins[i+1]<10**9 else 'inf'}"
# open distribution per member in window
for s in sorted(ent):
    os_=[x[5] for x in ent[s] if T0<=x[0]<=T1]
    print('member',s,'entries',len(os_),'open p50',pct(os_,.5),'p90',pct(os_,.9),'p99',pct(os_,.99),'max',max(os_))
# confirm by open at start, per member
for s in sorted(ent):
    c=collections.defaultdict(list)
    for x in rows[s]:
        if x[1]==10 and T0<=x[0]<=T1:
            st=x[0]-x[5]; c[b(open_at(s,st))].append(x[5])
    print('confirm',s,{k:(len(v),pct(v,.5),pct(v,.9)) for k,v in sorted(c.items(),key=lambda kv:int(kv[0].split('-')[0]))})
# raft out leg by leader open at send (raw, skew not corrected) per pair
raft_in={}
for s in ent:
    for x in ent[s]:
        if x[4]%64==1 and x[2]<128 and x[3]: raft_in[(x[2],x[3])]=(s,x[0],x[5])
out=collections.defaultdict(list); fo=collections.defaultdict(list)
for s in rows:
    for x in rows[s]:
        if x[1]==23 and T0<=x[0]<=T1 and (s,x[3]) in raft_in:
            callee,at,fopen=raft_in[(s,x[3])]
            out[b(open_at(s,x[0]))].append(at-x[0])
            fo[b(open_at(callee,at))].append(at-x[0])
print('out leg by SENDER open at send',{k:(len(v),pct(v,.5),pct(v,.9)) for k,v in sorted(out.items(),key=lambda kv:int(kv[0].split('-')[0]))})
print('out leg by CALLEE open at entry',{k:(len(v),pct(v,.5),pct(v,.9)) for k,v in sorted(fo.items(),key=lambda kv:int(kv[0].split('-')[0]))})
