exec(open('tests/fanout/votershed/b6.py').read().split('pairs=[]')[0])
def rate(s,t):
    return bisect.bisect_left(eat[s],t)-bisect.bisect_left(eat[s],t-2000)
agree=dis=0; ex=[]
for L,v in S.items():
    for i,a in enumerate(v):
        for b_ in v[i+1:]:
            if b_[0]-a[0]>20: break
            if b_[1]!=a[1]:
                if max(a[2],b_[2])>=1000 and abs(a[2]-b_[2])>=300:
                    ra,rb=rate(a[1],a[0]+a[2]),rate(b_[1],b_[0]+b_[2])
                    if ra==rb: break
                    if (a[2]>b_[2])==(ra>rb): agree+=1
                    else: dis+=1
                break
print('longer leg goes to the callee with more arrivals in the prior 2 s:',agree,'of',agree+dis)
# out leg vs leader open with follower rate controlled: low follower rate (<100/s) only
lo=collections.defaultdict(list)
for L,v in S.items():
    for t,callee,o,aw,lag in v:
        if rate(callee,t+o)<200: lo[min(open_at(L,t)//100*100,1000)].append(o)
print('follower quiet (<100/s): out leg by leader open',{k:(len(x),pct(x,.5)) for k,x in sorted(lo.items())})
