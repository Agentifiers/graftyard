import json, lzma, sys, bisect
# The two-round floor (`tests/m0/results/cut-trace.md`, "Run 5"): per /commit arrival on the leader (raw clock), the
# entry it rides under read-ahead (the second append after arrival) and the round of each of the two entries it
# spans, the one in flight and its own. Prints commit, round p50/p99, 2 x round p99, the two rounds' sum, and what
# the commit spends past them. Usage: floor.py TRACE.jsonl.xz LOAD_START_MS
path, t0 = sys.argv[1], int(sys.argv[2]); t1 = t0 + 600000
seen=set(); app={}; aply=[]; arr=[]
for l in lzma.open(path,'rt'):
    d=json.loads(l)
    if 'rows' not in d or 'learner' in d or d.get('site')!=0: continue
    for r in d['rows']:
        t=tuple(r)
        if t in seen: continue
        seen.add(t)
        if r[1]==15: app.setdefault(r[4], r[0])
        if r[1]==4: aply.append((r[0], r[4]))
        if r[1]==25 and r[4]%64==3 and t0<=r[0]<t1: arr.append(r[0])
aply.sort(); applied_at={}; prev=0
for at,i in aply:
    for x in range(prev+1,i+1): applied_at.setdefault(x,at)
    prev=max(prev,i)
ents=sorted((a,i) for i,a in app.items() if i in applied_at)
starts=[e[0] for e in ents]
rnd={i:applied_at[i]-a for a,i in ents}
def pc(x,q): x=sorted(x); return x[min(len(x)-1,int(len(x)*q))]
commit=[]; two=[]; n_ride=[]
for e in arr:
    j=bisect.bisect_right(starts,e)
    if j+1>=len(ents) or j==0: continue
    # the entry it rode: the first one appended after arrival whose apply comes after the arrival; under read-ahead
    # the usual one is the second append after arrival (the first was already frozen), as shares.py models
    a1,i1=ents[j]; a2,i2=ents[j+1]
    c=applied_at[i2]-e
    commit.append(c); two.append(rnd[i1]+rnd[i2])
R=[rnd[i] for _,i in ents if t0<=app[i]<t1]
print(f"modelled commit (second append after arrival) p50 {pc(commit,.5)} p99 {pc(commit,.99)}")
print(f"round p50 {pc(R,.5)} p99 {pc(R,.99)}; 2 x round p99 {2*pc(R,.99)}")
print(f"sum of the two rounds it spans p50 {pc(two,.5)} p99 {pc(two,.99)}")
over=[c-t for c,t in zip(commit,two)]; print(f"commit minus those two rounds p50 {pc(over,.5)} p99 {pc(over,.99)}")
