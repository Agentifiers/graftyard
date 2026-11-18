exec(open('tests/fanout/votershed/a3.py').read().split('tab=collections')[0])
# For leader->follower /raft calls with corrected out leg >= 1 s: the requests that entered the follower ahead, their open counts and routes
eidx={s:{x[4]//64:x for x in ent[s]} for s in ent}
res=collections.defaultdict(lambda: collections.Counter()); n=0; tot=0
opens=[]
for s in rows:
    for x in rows[s]:
        if x[1]==23 and T0<=x[0]<=T1 and (s,x[3]) in raft_in and leader(x[0])==s and (s,x[2]) in skew:
            callee,at,fopen=raft_in[(s,x[3])]
            o=at-x[0]-skew[(s,x[2])]
            if o<1000: continue
            n+=1
            i=bisect.bisect_left(eat[callee],at-o); j=bisect.bisect_left(eat[callee],at)
            for y in ent[callee][i:j]:
                tot+=1
                r=route(y[4]%64); edge=y[2]==255
                opens.append(y[5])
                for B in (32,64,128,256,512):
                    if edge and r in ('/read','/suspect','/outcome_by_request') and y[5]-1>=B: res[B]['shed']+=1
print('calls',n,'ahead total',tot,'per call',tot//max(1,n),'open of ahead p50',pct(opens,.5),'p90',pct(opens,.9),'p99',pct(opens,.99))
for B in sorted(res): print('B',B,'shed share of ahead %.2f'%(res[B]['shed']/tot))
