exec(open('tests/fanout/votershed/a3.py').read().split('tab=collections')[0])
# leader->follower out leg (uncorrected, per pair) by leader open at send; finer bins
fb=[0,25,50,75,100,150,200,300,500,1000,10**9]
def fbin(o):
    for i in range(len(fb)-1):
        if fb[i]<=o<fb[i+1]: return fb[i]
tab=collections.defaultdict(list)
for s in rows:
    for x in rows[s]:
        if x[1]==23 and T0<=x[0]<=T1 and (s,x[3]) in raft_in and leader(x[0])==s:
            callee,at,fopen=raft_in[(s,x[3])]
            tab[((s,x[2]),fbin(open_at(s,x[0])))].append(at-x[0])
pairs=sorted({k[0] for k in tab})
for p in pairs:
    base=pct(tab.get((p,0),[0]),.5)
    print(p,' '.join(f"{lo}:{len(tab[(p,lo)])}/{pct(tab[(p,lo)],.5)-base if tab.get((p,lo)) else None}/{pct(tab[(p,lo)],.9)-base if tab.get((p,lo)) else None}" for lo in fb[:-1] if (p,lo) in tab))
# share of time leader spends at open >= B, and share of edge read/suspect/obr arriving at open>=B, on leader and followers
for B in [64,128,192,256,384,512,1024]:
    out=[]
    for s in sorted(ent):
        e=[x for x in ent[s] if T0<=x[0]<=T1]
        cand=[x for x in e if x[2]==255 and route(x[4]%64) in ('/read','/suspect','/outcome_by_request')]
        shed=[x for x in cand if x[5]-1>=B]
        lead=[x for x in cand if leader(x[0])==s]; lshed=[x for x in lead if x[5]-1>=B]
        out.append(f"{s}: shed {len(shed)/len(cand):.2f} (as leader {len(lshed)/max(1,len(lead)):.2f}, n {len(lead)})")
    print('B',B,' | '.join(out))
