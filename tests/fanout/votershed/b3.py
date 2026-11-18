exec(open('tests/fanout/votershed/a1.py').read().split('# open distribution')[0])
for s in sorted(rows):
    E=[x for x in ent[s] if T0<=x[0]<=T1 and route(x[4]%64)=='/suspect']
    R=[x for x in rows[s] if x[1]==28 and T0<=x[0]<=T1]
    # per 10 s bucket where Entered has no arrival gaps
    eb=collections.Counter(x[0]//10000 for x in E); rb=collections.Counter(x[0]//10000 for x in R)
    allE=sorted(x[4]//64 for x in ent[s] if T0<=x[0]<=T1)
    print(s,'suspect entered',len(E),'reported',len(R),'ratio %.2f'%(len(R)/max(1,len(E))))
    print('  per 10s E/R:',' '.join(f"{eb[k]}/{rb.get(k,0)}" for k in sorted(eb)[:30]))
