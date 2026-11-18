import json, lzma, bisect, sys, random
# Replays B0-29 run 5's leader: measured /commit arrivals, measured rounds (append->apply, in order) and measured
# dependency-read-plus-decide times (cut->append of cuts taken at apply), under a freeze rule; commit = apply - arrival.
path, t0 = sys.argv[1], int(sys.argv[2]); t1=t0+600000
seen=set(); app={}; aply=[]; ct=[]; arr=[]
for l in lzma.open(path,'rt'):
    d=json.loads(l)
    if 'rows' not in d or 'learner' in d or d.get('site')!=0: continue
    for r in d['rows']:
        t=tuple(r)
        if t in seen: continue
        seen.add(t)
        if not (t0-5000<=r[0]<t1+5000): continue
        if r[1]==15: app.setdefault(r[4], r[0])
        if r[1]==4: aply.append((r[0],r[4]))
        if r[1]==29: ct.append((r[0], r[2]>=64))
        if r[1]==25 and r[4]%64==3 and t0<=r[0]<t1: arr.append(r[0])
aply.sort(); applied_at={}; prev=0
for at,idx in aply:
    for i in range(prev+1,idx+1): applied_at.setdefault(i,at)
    prev=max(prev,idx)
ct.sort(); cts=[c[0] for c in ct]; arr.sort()
idx=sorted(i for i in app if i in applied_at and t0<=app[i]<t1)
rounds=[applied_at[i]-app[i] for i in idx]
D=[]
for a,b in zip(idx,idx[1:]):
    k=bisect.bisect_right(cts,app[b])-1
    if k>=0 and not ct[k][1]: D.append(app[b]-cts[k])
def pc(xs,q): xs=sorted(xs); return xs[min(len(xs)-1,int(len(xs)*q))]
def sim(rule, seed=1):
    rnd=random.Random(seed); lat=[]; reads=0; misses=0
    i=0; t=arr[0]; R=150.0; Dm=125.0; k=0; nents=0
    pending=None  # (append, apply)
    while i < len(arr):
        if pending is None:
            # nothing in flight: cut at max(t, next arrival), read, append
            cut=max(t, arr[i]); d=rnd.choice(D); reads+=1
            j=bisect.bisect_right(arr,cut); batch=range(i,j); i=j
            ap=cut+d; r=rounds[k%len(rounds)]; k+=1; pl=ap+r
            for x in batch: lat.append(pl-arr[x])
            R=0.9*R+0.1*r; Dm=0.9*Dm+0.1*d; pending=(ap,pl); continue
        ap,pl=pending; d=rnd.choice(D)
        f, nreads = rule(ap, pl, R, Dm, d)
        reads+=nreads
        if f is None or f>=pl or bisect.bisect_right(arr,f)<=i:
            # missed (or nothing queued at freeze): cut at apply or the next arrival after it
            misses+= f is not None and f>=pl
            pending=None; t=pl; continue
        j=bisect.bisect_right(arr,f); batch=range(i,j); i=j
        nap=max(pl, f+d); r=rounds[k%len(rounds)]; k+=1; npl=nap+r
        for x in batch: lat.append(npl-arr[x])
        R=0.9*R+0.1*r; Dm=0.9*Dm+0.1*d; pending=(nap,npl)
    return pc(lat,.5), pc(lat,.99), len(lat), reads, misses
rules = {
 'current: append + mean round - mean read': lambda ap,pl,R,Dm,d: (ap+max(0,R-Dm),1),
 'oracle: apply - this read (unreachable)': lambda ap,pl,R,Dm,d: (max(ap,pl-d),1),
}
for I in (25,50,75):
    def roll(ap,pl,R,Dm,d,I=I):
        # a read asked every I ms from the append; at apply the batch is the commands before the latest read that
        # has answered, else the first read still running is awaited.
        asks=[ap+j*I for j in range(0,1000) if ap+j*I<pl]
        done=[a for a in asks if a+d<=pl]
        f=done[-1] if done else asks[0]
        return f, len(asks)
    rules[f'rolling read every {I} ms'] = roll
for q in (0.6,0.8):
    rules[f'append + {q} x mean round - mean read'] = (lambda q: lambda ap,pl,R,Dm,d: (ap+max(0,q*R-Dm),1))(q)
print(f"arrivals {len(arr)}, entries {len(rounds)}, read samples {len(D)} (p50 {pc(D,.5)} p99 {pc(D,.99)}), round p50 {pc(rounds,.5)} p99 {pc(rounds,.99)}")
for name, rule in rules.items():
    out=[sim(rule,s) for s in (1,2,3)]
    print(f"{name}: commit p50 {sum(o[0] for o in out)//3} p99 {sum(o[1] for o in out)//3}, reads/entry-ish {out[0][3]}, misses {out[0][4]}")
print("--- variants")
extra = {
 'append + mean round - 1.5 x mean read': lambda ap,pl,R,Dm,d: (ap+max(0,R-1.5*Dm),1),
 'append + mean round - 2 x mean read': lambda ap,pl,R,Dm,d: (ap+max(0,R-2*Dm),1),
 'freeze at append (wait 0)': lambda ap,pl,R,Dm,d: (ap,1),
}
for name, rule in extra.items():
    out=[sim(rule,s) for s in (1,2,3)]
    print(f"{name}: commit p50 {sum(o[0] for o in out)//3} p99 {sum(o[1] for o in out)//3}")
D0=list(D)
for s in (0.5,0.0):
    D[:]=[x*s for x in D0]
    for name in ('current: append + mean round - mean read','oracle: apply - this read (unreachable)'):
        out=[sim(rules[name],z) for z in (1,2,3)]
        print(f"reads x{s}: {name}: p50 {sum(o[0] for o in out)//3} p99 {sum(o[1] for o in out)//3}")
D[:]=D0
R0=list(rounds)
rounds[:]=[min(r,200) for r in R0]
out=[sim(rules['current: append + mean round - mean read'],z) for z in (1,2,3)]
print(f"rounds capped at 200 ms: current rule p50 {sum(o[0] for o in out)//3} p99 {sum(o[1] for o in out)//3}")
print("--- hedged dependency read: min(d1, H + d2), each d from the at-apply samples")
for H in (100,150,200,300):
    hr=random.Random(7); D[:]=[min(hr.choice(D0), H+hr.choice(D0)) for _ in range(5000)]
    out=[sim(rules['current: append + mean round - mean read'],z) for z in (1,2,3)]
    print(f"hedge at {H} ms: read p99 {pc(D,.99)}; current rule p50 {sum(o[0] for o in out)//3} p99 {sum(o[1] for o in out)//3}")
D[:]=D0
