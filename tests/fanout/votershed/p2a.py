#!/usr/bin/env python3
"""Point 2 of the voter-shed review (voter-shed-design.md, "Point 2"): m47 leader open requests by class, and the
leader->follower round trip split. Needs /tmp/shed/rows.pkl (prep.py). numpy/scipy: uv run -q --with numpy --with scipy."""
import pickle,bisect,collections,datetime,json
rows=pickle.load(open('/tmp/shed/rows.pkl','rb'))
iso=lambda s:int(datetime.datetime.fromisoformat(s.replace('Z','+00:00')).timestamp()*1000)
T0,T1=iso('2026-11-18T01:00:18Z'),iso('2026-11-18T01:05:18Z')
pct=lambda xs,q:(sorted(xs)[min(len(xs)-1,int(q*len(xs)))] if xs else None)
R=["/raft","/read","/commit","/commit_many","/outcome","/outcome_by_request","/suspect"]
route=lambda c:R[c-1] if 0<c<=7 else 'other'
samples=[]
for l in open('/home/user/workspace/repo/tests/fanout/raw/terms-m47b33.jsonl'):
    r=json.loads(l)
    if r.get('members'): samples.append((iso(r['at']),{m[0]:m[1] for m in r['members']}))
st=[t for t,_ in samples]
def leader(t):
    i=max(0,bisect.bisect_right(st,t)-1); return next((k for k,v in samples[i][1].items() if v=='leader'),None)
for s in sorted(rows):
    v=rows[s]
    ent={x[4]//64:x for x in v if x[1]==25}
    body={}
    for x in v:
        if x[1]==26: body[x[3]]=x[0]         # ReadQuery batch=arrival
    raftbody={}
    rin={(x[2],x[3]):x for x in v if x[1]==25 and x[4]%64==1 and x[3]}
    for x in v:
        if x[1]==20 and (x[2],x[3]) in rin: raftbody[rin[(x[2],x[3])][4]//64]=x[0]
    d=collections.defaultdict(list)
    for a,x in ent.items():
        if not(T0<=x[0]<=T1) or leader(x[0])!=s: continue
        r=route(x[4]%64)
        if r=='/read' and a in body: d[('/read',min(x[5]//250*250,1500))].append(body[a]-x[0])
        if r=='/raft' and a in raftbody: d[('/raft',min(x[5]//250*250,1500))].append(raftbody[a]-x[0])
    if d:
        print('member',s,'as leader: entry->body-read ms by open at entry (n, p50, p90)')
        for k in sorted(d): print('  ',k,len(d[k]),pct(d[k],.5),pct(d[k],.9))
