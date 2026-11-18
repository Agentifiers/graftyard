import json, lzma, bisect, sys, collections
# B0-29 run 5's tail commands (commit over p99 by the replay of each /commit to the entry that carried it): which
# freeze each rode and whether the batch it joined was cut early or at an apply.
path, t0 = sys.argv[1], int(sys.argv[2]); t1=t0+600000
seen=set(); app={}; aply=[]; cuts=[]; arr=[]
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
        if r[1]==29: cuts.append((r[0], r[2]>=64))
        if r[1]==25 and r[4]%64==3 and t0<=r[0]<t1: arr.append(r[0])
aply.sort(); applied_at={}; prev=0
for at,idx in aply:
    for i in range(prev+1,idx+1): applied_at.setdefault(i,at)
    prev=max(prev,idx)
cuts.sort(); ct=[c[0] for c in cuts]
ents=sorted((a,i) for i,a in app.items() if i in applied_at)
ea=[a for a,_ in ents]
def pc(xs,q): xs=sorted(xs); return xs[min(len(xs)-1,int(len(xs)*q))]
rows=[]
for e in arr:
    k=bisect.bisect_left(ct,e)          # first cut at or after arrival takes it
    if k>=len(ct): continue
    c=ct[k]; j=bisect.bisect_left(ea,c) # first append at or after the cut carries it
    if j>=len(ents): continue
    a,i=ents[j]; rows.append((applied_at[i]-e, c-e, a-c, applied_at[i]-a, cuts[k][1]))
p99=pc([r[0] for r in rows],.99); tail=[r for r in rows if r[0]>=p99]
print(f"commands {len(rows)}; replayed commit p50 {pc([r[0] for r in rows],.5)} p99 {p99}")
for name,g in (("all",rows),("tail",tail)):
    T=sum(r[0] for r in g)
    print(name, len(g), "arrival->cut %.0f%%, cut->append %.0f%%, round %.0f%%; early-cut batches %d of %d" % (
        100*sum(r[1] for r in g)/T, 100*sum(r[2] for r in g)/T, 100*sum(r[3] for r in g)/T, sum(r[4] for r in g), len(g)))
print("tail arrival->cut p50/max", pc([r[1] for r in tail],.5), max(r[1] for r in tail), "cut->append p50/max", pc([r[2] for r in tail],.5), max(r[2] for r in tail))
