"""holdmodel.py TSAVE LOAD_START_ISO OUTCOME_PER_COMMIT (b033-rebase.md, item 2). Hold model. W = each /commit's trace wait (arrival -> its cut's entry applied, bycut's join, all /commit inputs).
The leader answers `admitted` for 52% (sabA) of /commits though only 15% of W pass 1 s, so the ticket resolves later than
`Applied` by the leader's view: shift W by delta, fitted so P(W+delta > 1 s) = measured /outcome per /commit."""
import bisect, collections, json, lzma, sys, datetime
P, F, ASK = sys.argv[1], sys.argv[2], float(sys.argv[3])
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
T0 = iso(F); T1 = T0 + 300000
seen=set(); arr=[]; app={}; aply=[]; cuts=[]; inputs=0
for l in lzma.open(P,'rt'):
    d=json.loads(l)
    if 'rows' not in d or d.get('learner') is not None or d.get('site')!=0: continue
    for r in d['rows']:
        t=tuple(r)
        if t in seen: continue
        seen.add(t)
        if not (T0-10000<=r[0]<T1+10000): continue
        k=r[1]
        if k==25 and T0<=r[0]<T1:
            inputs+=1
            if int(float(r[4]))%64==3: arr.append(r[0])
        elif k==15: app.setdefault(r[4],r[0])
        elif k==4: aply.append((r[0],r[4]))
        elif k==29: cuts.append(r[0])
aply.sort(); at={}; prev=0
for a,i in aply:
    for j in range(prev+1,i+1): at.setdefault(j,a)
    prev=max(prev,i)
ents=sorted((a,i) for i,a in app.items() if i in at); eat=[a for a,_ in ents]; cuts.sort()
S=[]
for e in sorted(arr):
    c=bisect.bisect_left(cuts,e)
    if c>=len(cuts): continue
    j=bisect.bisect_left(eat,cuts[c])
    if j>=len(ents): continue
    S.append((e,at[ents[j][1]]-e))
W=[w for _,w in S]; n=len(W); lam=len(arr)/300
over=lambda d,h: sum(w+d>h for w in W)/n
lo,hi=0,5000
for _ in range(40):
    m=(lo+hi)/2
    if over(m,1000)>ASK: hi=m
    else: lo=m
d=round(lo); Wd=sorted(w+d for w in W); pc=lambda q: Wd[min(n-1,int(q*n))]
print(f'/commit {lam:.0f}/s, leader inputs {inputs/300:.0f}/s; fitted delta {d} ms; resolve wait W+delta p50 {pc(.5)} p90 {pc(.9)} p99 {pc(.99)}')
base=over(d,1000)
for h in (1000,1500,2000,3000):
    a=over(d,h); held=lam*sum(min(w+d,h) for w in W)/n/1000; polls=lam*sum(min(max(0,w+d-h),10000) for w in W)/n/1000
    ev=[]
    for (e,w) in S: ev+=[(e,1),(e+min(w+d,h),-1)]
    ev.sort(); c=pk=0
    for _,x in ev: c+=x; pk=max(pk,c)
    print(f'  hold {h/1000:3.1f} s: over {100*a:4.1f}% -> /outcome asks {a:.2f}/commit, {lam*a:5.1f}/s ({lam*(a-base):+6.1f}/s); leader inputs ~{inputs/300+lam*(a-base):4.0f}/s ({100*lam*(a-base)/(inputs/300):+5.1f}%) | held /commit at the leader mean {held:5.1f} peak {pk:4} | held /outcome polls mean {polls:5.1f}')
