#!/usr/bin/env python3
# Replay of edge routing over gates' learner traces (narrow-reads-3, the herding guard). Each read at time t to learner L
# waits as L's traced batches say: it leaves in the next 50 ms batch, whose round trip and read index are taken from
# the traced batch nearest that send, and is granted once L applied that index; past FOLLOWER_WAIT (1 s) the member
# answers late at 1 s and the edge tries the next member (3 at most). OV ms of edge stages outside the tier read.
import json,sys,random,bisect,collections,math
def pct(xs,q):
    xs=sorted(xs); return xs[min(len(xs)-1,int(q*len(xs)))]
def load(g):
    D=json.load(open(f"tests/nr3/raw/trace-{g}.json"))
    f0=D["f0"][0][0]; end=D["f0"][-1][0]
    G={}
    for site in sorted(k for k in D if k.startswith("l")):
        sent={}; B=[]; ap=[]
        for at,kind,peer,batch,index,ms in D[site]:
            if kind==1: sent.setdefault(batch,[]).append(at)
            elif kind==3 and ms==0 and sent.get(batch):
                s=sent[batch].pop(0)
                if index: B.append((s,at,index))
            elif kind==4: ap.append((at,index))
        B.sort(); ap.sort()
        G[site]=(B,[b[0] for b in B],ap,[a[0] for a in ap])
    return G,f0,end
def wait(G,site,t,rnd):
    B,S,ap,AT=G[site]; send=t+rnd.uniform(0,50); j=min(bisect.bisect_left(S,send),len(B)-1)
    if j>0 and abs(S[j-1]-send)<abs(S[j]-send): j-=1
    s,r,idx=B[j]; res=send+(r-s)
    k=bisect.bisect_left(AT,res)
    if k>0 and ap[k-1][1]>=idx: return res-t
    while k<len(ap) and ap[k][1]<idx: k+=1
    return ap[k][0]-t if k<len(ap) else 10**6
W=1000; CALL=2000; XP=False; DW=0.25; K=4
def run(G,f0,end,policy,iso=4,rate=200,OV=0,seed=11,a=0.2,margin=200,spread=2,stale=10000,late_ms=1000,explore=20):
    rnd=random.Random(seed); ks=list(G)
    st=[dict(mean={},dev={},seen={}) for _ in range(iso)]
    out=[];share=collections.Counter();n=0;lates=collections.Counter()
    t=f0+60000
    while t<end-15000:
        t+=rnd.expovariate(rate/1000); I=st[rnd.randrange(iso)]; n+=1
        mean,dev,seen=I['mean'],I['dev'],I['seen']
        if policy=='current':
            sc={k:mean.get(k,0.0) for k in ks}; best=min(sc.values())
            if rnd.random()<1/explore: first=rnd.choice(ks)
            else: first=rnd.choice([k for k in ks if sc[k]<=best+margin])
            order=[first]+sorted([k for k in ks if k!=first],key=lambda k:sc[k])
        else:
            # guard: a learner unsampled for `stale` ms counts as untimed; untimed learners are eligible but do not set
            # the fastest; eligible = within margin of the fastest timed one, padded to `spread` by score with learners
            # whose score is under late_ms; reads go at random among the eligible; no per-read exploring.
            fresh={k:(mean[k]+(K*dev[k] if policy=="guard-dev" else 0)) for k in ks if k in mean and t-seen[k]<stale}
            best=min(fresh.values()) if fresh else 0
            elig=[k for k in ks if k not in fresh or fresh[k]<=best+margin]
            if policy!='top2':
                rest=sorted([k for k in fresh if k not in elig and fresh[k]<late_ms],key=lambda k:fresh[k])
                elig+=rest[:max(0,spread-len(elig))]
            else:
                elig=sorted(ks,key=lambda k:fresh.get(k,0))[:2]
            first=rnd.choice(elig)
            if XP and rnd.random()<1/explore: first=rnd.choice(ks)
            order=[first]+sorted([k for k in ks if k!=first],key=lambda k:(k not in elig, fresh.get(k,0)))
        tot=0
        for i,site in enumerate(order[:3]):
            w=wait(G,site,t+tot,rnd); ok=w<=W
            s=w if ok else CALL
            if site in mean and (policy=='current' or t-seen.get(site,0)<stale):
                d=s-mean[site]; mean[site]+=a*d; dev[site]=dev.get(site,0)+DW*(abs(d)-dev.get(site,0))
            else: mean[site]=s; dev[site]=0
            seen[site]=t
            if i==0: share[site]+=1
            if ok: tot+=w; break
            lates[site]+=1; tot+=W
        out.append(tot+OV)
    return dict(p50=int(pct(out,.5)),p99=int(pct(out,.99)),over2s="%.2f%%"%(100*sum(x>2000 for x in out)/n),late="%.2f%%"%(100*sum(lates.values())/n),
                share={k:round(100*v/n,1) for k,v in sorted(share.items())},maxshare=round(100*max(share.values())/n,1))
if __name__=="__main__":
    # `current` is routing by the average alone with one read in 20 at random (a3a21439); `guard-dev` is what
    # crates/lookup builds: score = average + ROUTE_DEV (4) mean deviations, both at ROUTE_WEIGHT (0.2), eligible =
    # untimed or within ROUTE_MARGIN (200) of the best, padded to ROUTE_SPREAD (3) under ROUTE_LATE (1,000), a learner
    # unseen for ROUTE_STALE (10 s) untimed. 4 edge isolates, 200 reads/s, 120 ms of edge stages outside the tier read.
    #   python3 tests/nr3/route.py gate13 gate14 gate15
    K=4; DW=0.2
    for g in sys.argv[1:]:
        G,f0,end=load(g)
        for pol,kw in (('current',{}),('guard-dev',dict(spread=3,stale=10000))):
            r=run(G,f0,end,pol,iso=4,OV=120,**kw)
            print(g,pol,'p99',r['p99'],'over 2 s',r['over2s'],'late',r['late'],'busiest',r['maxshare'],r['share'],flush=True)
