exec(open('tests/fanout/votershed/a1.py').read().split('# open distribution')[0])
skew={(0, 1): -395, (0, 2): 226, (1, 0): -1135, (1, 2): -105, (2, 0): -1213, (2, 1): 67}
raft_in={}
for s in ent:
    for x in ent[s]:
        if x[4]%64==1 and x[2]<128 and x[3]: raft_in[(x[2],x[3])]=(s,x[0],x[5])
lag={s:[(x[0],x[5]) for x in rows[s] if x[1]==8] for s in rows}
lat={s:[t for t,_ in lag[s]] for s in rows}
def loopl(s,t):
    i=bisect.bisect_right(lat[s],t+200); j=bisect.bisect_left(lat[s],t-1000)
    return max([m for _,m in lag[s][j:i]]+[0])
# terms: who led when (from rows 'leader' field? use terms file)
import json
samples=[]
for line in open('tests/fanout/raw/terms-m47b33.jsonl'):
    r=json.loads(line)
    if r.get('members'): samples.append((iso(r['at']),{m[0]:m[1] for m in r['members']}))
st=[t for t,_ in samples]
def leader(t):
    i=max(0,bisect.bisect_right(st,t)-1); return next((k for k,v in samples[i][1].items() if v=='leader'),None)
tab=collections.defaultdict(list); tabf=collections.defaultdict(list); tabl=collections.defaultdict(list)
for s in rows:
    for x in rows[s]:
        if x[1]==23 and T0<=x[0]<=T1 and (s,x[3]) in raft_in and (s,x[2]) in skew:
            callee,at,fopen=raft_in[(s,x[3])]
            o=at-x[0]-skew[(s,x[2])]
            role='L' if leader(x[0])==s else 'F'
            tab[(role,b(open_at(s,x[0])))].append(o)
            tabf[(role,b(fopen))].append(o)
            tabl[(role,'lag>=100' if x[5]>=100 else 'lag<100')].append(o)
key=lambda kv:(kv[0][0],int(kv[0][1].split('-')[0]) if kv[0][1][0].isdigit() else 0)
print('corrected out leg by SENDER open at send (role of sender)')
for k,v in sorted(tab.items(),key=key): print('  ',k,len(v),'p50',pct(v,.5),'p90',pct(v,.9))
print('corrected out leg by CALLEE open at entry (role of sender)')
for k,v in sorted(tabf.items(),key=key): print('  ',k,len(v),'p50',pct(v,.5),'p90',pct(v,.9))
for k,v in sorted(tabl.items()): print('  ',k,len(v),'p50',pct(v,.5),'p90',pct(v,.9))
