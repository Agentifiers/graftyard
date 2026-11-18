exec(open('tests/fanout/votershed/a3.py').read().split('tab=collections')[0])
left={}; ans={}
for s in rows:
    for x in rows[s]:
        if x[1]==24: left[(x[2],x[3])]=x[0]     # callee s: CallLeft peer=caller, id
        if x[1]==21 and x[3]: ans[(s,x[3])]=(x[0],x[4])  # caller s: answer arrived at, index=callee's left stamp
D=collections.defaultdict(list)
for s in rows:
    for x in rows[s]:
        if x[1]==23 and T0<=x[0]<=T1 and (s,x[3]) in raft_in and (s,x[3]) in ans and (s,x[2]) in skew:
            callee,at,_=raft_in[(s,x[3])]
            k=skew[(s,x[2])]
            out=at-x[0]-k
            a_at,lft=ans[(s,x[3])]
            back=a_at-lft+k
            role='L' if leader(x[0])==s else 'F'
            bucket='out<200' if out<200 else 'out200-1000' if out<1000 else 'out1-3s' if out<3000 else 'out3s+'
            D[(role,bucket)].append((out,back,lft-at))
for k in sorted(D):
    v=D[k]
    print(k,'n',len(v),'out p50',pct([a[0] for a in v],.5),'back p50',pct([a[1] for a in v],.5),'p90',pct([a[1] for a in v],.9),'handler p50',pct([a[2] for a in v],.5))
