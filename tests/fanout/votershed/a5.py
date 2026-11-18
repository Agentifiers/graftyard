exec(open('tests/fanout/votershed/a3.py').read().split('tab=collections')[0])
conf={s:sorted((x[0]-x[5],x[0]) for x in rows[s] if x[1]==10) for s in rows}
def merge(sp):
    m=[]
    for a,b_ in sp:
        if m and a<=m[-1][1]: m[-1][1]=max(m[-1][1],b_)
        else: m.append([a,b_])
    return m
mg={s:merge(conf[s]) for s in rows}; mgs={s:[x[0] for x in mg[s]] for s in rows}
def gate(s,t):
    i=bisect.bisect_right(mgs[s],t)-1
    return mg[s][i][1]-t if i>=0 and mg[s][i][0]<=t<mg[s][i][1] else 0
fb=[0,50,100,200,300,500,1000,10**9]
def fbin(o):
    for i in range(len(fb)-1):
        if fb[i]<=o<fb[i+1]: return fb[i]
byL=collections.defaultdict(list); byF=collections.defaultdict(list); byFlag=collections.defaultdict(list)
for s in rows:
    for x in rows[s]:
        if x[1]==23 and T0<=x[0]<=T1 and (s,x[3]) in raft_in and leader(x[0])==s and (s,x[2]) in skew:
            callee,at,fopen=raft_in[(s,x[3])]
            o=at-x[0]-skew[(s,x[2])]
            g=gate(s,x[0]); rest=o-g-x[5]
            byL[fbin(open_at(s,x[0]))].append((o,g,x[5],rest))
            # follower load: requests entered callee in the 2 s before at
            i=bisect.bisect_left(eat[callee],at-2000); j=bisect.bisect_left(eat[callee],at)
            rate=(j-i)/2
            byF[fbin(rate)].append((o,g,x[5],rest))
            fl=loopl(callee,at); byFlag['flag>=100' if fl>=100 else 'flag<100'].append((o,g,x[5],rest))
def show(name,d):
    print(name)
    for k in sorted(d):
        v=d[k]; print(f"   {k}: n {len(v)} out p50 {pct([a[0] for a in v],.5)} gate p50 {pct([a[1] for a in v],.5)} mean {sum(a[1] for a in v)//len(v)} lag mean {sum(a[2] for a in v)//len(v)} rest p50 {pct([a[3] for a in v],.5)} mean {sum(a[3] for a in v)//len(v)}")
show('leader->follower by LEADER open at send',byL)
show('by FOLLOWER arrival rate /s in prior 2 s',byF)
show('by follower ticker lag near entry',byFlag)
