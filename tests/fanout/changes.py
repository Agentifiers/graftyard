#!/usr/bin/env python3
"""changes.py: B0-33 m53's leader changes, each with the StepDown alarm (Workers Logs) that ended the term if any, the
`started` verdicts before it, and the edge colos that reported `suspect` to each member in the 12 s before (Reported
28, colo packed as ASCII in a u32). Paths and alarms are m53's; edit them for another row."""
import json,collections,datetime,bisect
iso=lambda s:int(datetime.datetime.fromisoformat(s.replace("Z","+00:00")).timestamp()*1000)
f=lambda t:datetime.datetime.fromtimestamp(t/1000,datetime.UTC).strftime('%M:%S.%f')[:-3]
V=["leader","resumed","started","bootstrapping","promised","busy","won","lost","mayNotLead"]
name=lambda n: n.to_bytes(4,'big').lstrip(b'\0').decode() if n else 'site'
rep=collections.defaultdict(list)
for line in open('tests/fanout/raw/tsave-m53b33.jsonl'):
    d=json.loads(line)
    if 'rows' not in d or d.get('learner') is not None: continue
    for x in d['rows']:
        if x[1]==28: rep[d['site']].append((x[0],x[3],name(x[4]),V[x[5]] if x[5]<9 else '?'))
for s in rep: rep[s]=sorted(set(rep[s]))
alarms=[('21:48:43.909',2,'CommitErrors'),('21:49:13.631',0,'Confirm'),('21:50:50.809',0,'CommitErrors'),('21:51:01.925',2,'CommitErrors'),('21:51:31.707',0,'CommitErrors'),('21:52:29.976',2,'CommitErrors')]
al=[(iso('2026-11-18T'+t+'Z'),s,w) for t,s,w in alarms]
terms=[]; last=None
for l in open('tests/fanout/raw/terms-m53b33.jsonl'):
    try: r=json.loads(l)
    except: continue
    if 'members' not in r: continue
    lead=[(m[0],m[2]) for m in r['members'] if m[1]=='leader']
    if lead and lead!=last: terms.append((iso(r['at']),lead[0])); last=lead
print('new leader seen | the term ended by: alarm within 12 s before? | started verdicts in the 12 s before (member, colos seen in that member\'s reports 10 s earlier)')
for i,(t,(s,term)) in enumerate(terms[1:],1):
    pt=terms[i-1][0]; prev=terms[i-1][1]
    a=[w+'@'+str(x) for (ta,x,w) in al if pt<=ta<=t and x==prev[0]]
    st=[]
    for m in (0,1,2):
        for (tr,tm,c,v) in rep[m]:
            if t-12000<=tr<=t and v=='started': st.append((m,f(tr)))
    colos={}
    for m in (0,1,2):
        cs={c for (tr,tm,c,v) in rep[m] if t-12000<=tr<=t and c!='site'}
        colos[m]=''.join(sorted(x[0] for x in cs))
    print(f"{f(t)} site {s} term {term} (prev site {prev[0]} t{prev[1]}): alarm {a or '-'}; started {st[:3] or '-'}; colos/member {colos}")
