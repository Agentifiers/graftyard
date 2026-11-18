exec(open('tests/fanout/votershed/a3.py').read().split('tab=collections')[0])
# Backlog periods at followers: when a leader->follower /raft call's corrected out leg >= 1000, the span [at-o, at] at the callee is backlog.
spans=collections.defaultdict(list)
for s in rows:
    for x in rows[s]:
        if x[1]==23 and T0<=x[0]<=T1 and (s,x[3]) in raft_in and leader(x[0])==s and (s,x[2]) in skew:
            callee,at,_=raft_in[(s,x[3])]
            o=at-x[0]-skew[(s,x[2])]
            if o>=1000: spans[callee].append((at-o,at))
def inspan(s,t):
    return any(a<=t<=b_ for a,b_ in spans[s])
for s in sorted(spans):
    sp=sorted(spans[s]); merged=[]
    for a,b_ in sp:
        if merged and a<=merged[-1][1]: merged[-1][1]=max(merged[-1][1],b_)
        else: merged.append([a,b_])
    starts=[m[0] for m in merged]
    def ins(t):
        i=bisect.bisect_right(starts,t)-1
        return i>=0 and t<=merged[i][1]
    e=ent[s]; d=collections.defaultdict(list)
    for y,z in zip(e,e[1:]):
        if T0<=y[0]<=T1 and ins(y[0]) and z[4]//64==y[4]//64+1:
            d[(route(y[4]%64),'edge' if y[2]==255 else 'peer')].append(z[0]-y[0])
    tot=sum(sum(v) for v in d.values()); n=sum(len(v) for v in d.values())
    secs=sum(m[1]-m[0] for m in merged)/1000
    print(f"callee {s}: backlog {secs:.0f} s, entries {n} ({n/secs:.0f}/s), mean gap {tot/max(1,n):.2f} ms")
    for k,v in sorted(d.items(),key=lambda kv:-sum(kv[1]))[:7]:
        print(f"   {k}: n {len(v)} mean {sum(v)/len(v):.2f} ms p50 {pct(v,.5)} p90 {pct(v,.9)} share of time {sum(v)/tot:.2f}")
