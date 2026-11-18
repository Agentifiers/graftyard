exec(open('tests/fanout/votershed/a1.py').read().split('# open distribution')[0])
for s in sorted(ent):
    e=[x for x in ent[s] if T0<=x[0]<=T1]
    sec=collections.Counter(x[0]//1000 for x in e)
    rates=[sec.get(t,0) for t in range(T0//1000,T1//1000)]
    byr=collections.Counter(route(x[4]%64) for x in e); n=len(e)
    edge=sum(1 for x in e if x[2]==255)
    print(s,'rate/s p50',pct(rates,.5),'p90',pct(rates,.9),'max',max(rates),'edge share %.3f'%(edge/n),{k:round(v/n,3) for k,v in byr.most_common(7)})
    # edge /raft? from by route for edge-only
    eb=collections.Counter(route(x[4]%64) for x in e if x[2]==255)
    print('   edge by route per s',{k:round(v/300,1) for k,v in eb.most_common(8)})
    mb=collections.Counter(route(x[4]%64) for x in e if x[2]!=255)
    print('   member by route per s',{k:round(v/300,1) for k,v in mb.most_common(8)}, 'from',collections.Counter(x[2] for x in e if x[2]!=255).most_common(6))
