exec(open('tests/fanout/votershed/a3.py').read().split('tab=collections')[0])
# backlog spans per callee from leader /raft calls with corrected out >= 1000
spans=collections.defaultdict(list)
for s in rows:
    for x in rows[s]:
        if x[1]==23 and T0<=x[0]<=T1 and (s,x[3]) in raft_in and (s,x[2]) in skew:
            callee,at,_=raft_in[(s,x[3])]
            o=at-x[0]-skew[(s,x[2])]
            if o>=1000: spans[callee].append((at-o,at))
for s in sorted(rows):
    sp=sorted(spans[s]); m=[]
    for a,b_ in sp:
        if m and a<=m[-1][1]: m[-1][1]=max(m[-1][1],b_)
        else: m.append([a,b_])
    st=[x[0] for x in m]
    ins=lambda t:(lambda i: i>=0 and t<=m[i][1])(bisect.bisect_right(st,t)-1)
    E=[x for x in ent[s] if T0<=x[0]<=T1]
    arr={x[4]//64:x for x in E}
    rq=set(x[3] for x in rows[s] if x[1]==26)    # ReadQuery batch = arrival
    rp_t=[x[0] for x in rows[s] if x[1]==28]
    for name,sel in (('in backlog',lambda x: ins(x[0])),('outside',lambda x: not ins(x[0]))):
        reads=[x for x in E if route(x[4]%64)=='/read' and sel(x)]
        ok=sum(1 for x in reads if x[4]//64 in rq)
        print(s,name,'/read entered',len(reads),'body read (ReadQuery) %.2f'%(ok/max(1,len(reads))))
