exec(open('tests/fanout/votershed/a1.py').read().split('# open distribution')[0])
V=["leader","resumed","started","bootstrapping","promised","busy","won","lost","mayNotLead"]
for s in sorted(rows):
    rep=sorted(x for x in rows[s] if x[1]==28 and T0<=x[0]<=T1)
    n=len(rep); vc=collections.Counter(V[x[5]] if x[5]<len(V) else x[5] for x in rep)
    colos=collections.Counter(x[4] for x in rep)
    for G in (1000,2000,5000):
        last={}; step=0
        for x in rep:
            k=(x[4],x[2],x[3]); v=V[x[5]] if x[5]<len(V) else '?'
            if v!='promised' or k not in last or x[0]-last[k]>=G:
                step+=1; last[k]=x[0]
        print(f"member {s}: reports {n} ({n/300:.0f}/s), stepped with G={G}: {step} ({step/300:.1f}/s, {step/n:.3f})")
    print('   verdicts',dict(vc),'colos',len(colos),'unknown(0) share %.3f'%(colos.get(0,0)/max(1,n)),'top',colos.most_common(5))
