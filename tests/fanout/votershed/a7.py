exec(open('tests/fanout/votershed/a5.py').read().split('fb=[0')[0])
fb=[0,25,50,75,100,125,150,200,250,300,400,500,750,1000,10**9]
def fbin(o):
    for i in range(len(fb)-1):
        if fb[i]<=o<fb[i+1]: return fb[i]
outL=collections.defaultdict(list); confL=collections.defaultdict(list); lagL=collections.defaultdict(list)
for s in rows:
    for x in rows[s]:
        if x[1]==23 and T0<=x[0]<=T1 and (s,x[3]) in raft_in and leader(x[0])==s and (s,x[2]) in skew:
            callee,at,_=raft_in[(s,x[3])]
            outL[fbin(open_at(s,x[0]))].append(at-x[0]-skew[(s,x[2])])
        if x[1]==10 and T0<=x[0]<=T1 and leader(x[0]-x[5])==s:
            confL[fbin(open_at(s,x[0]-x[5]))].append(x[5])
        if x[1]==8 and T0<=x[0]<=T1 and leader(x[0])==s:
            lagL[fbin(open_at(s,x[0]))].append(x[5])
print('bin | out-leg n p50 p90 | confirm n p50 p90 | ticker lag n p50 p90')
for k in fb[:-1]:
    o,c,l=outL.get(k,[]),confL.get(k,[]),lagL.get(k,[])
    print(f"{k:5} | {len(o):4} {pct(o,.5)} {pct(o,.9)} | {len(c):3} {pct(c,.5)} {pct(c,.9)} | {len(l):3} {pct(l,.5)} {pct(l,.9)}")
# share of the leader's time at open>=B (time-weighted by entry rows ~ approximate with seconds)
for s in sorted(ent):
    e=[x for x in ent[s] if T0<=x[0]<=T1]
    F=[x[5] for x in e if leader(x[0])!=s]; L=[x[5] for x in e if leader(x[0])==s]
    print(s,'as follower open p50/p90/p99',pct(F,.5),pct(F,.9),pct(F,.99),'n',len(F),'| as leader',pct(L,.5),pct(L,.9),pct(L,.99),'n',len(L))
