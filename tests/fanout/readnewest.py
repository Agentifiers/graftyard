# read-newest's replay (`decisions/DECISIONS.md`, "`read-newest`"): a learner's ReadIndex calls to the leader (site 0)
# from a tsave trace's learner rows (kind 2 IndexEnded, kind 11 Hedge, kind 1 IndexSent), under main's rule and under
# read-newest's, holding every call's traced time, so the leader's lighter load is not credited. Under the new rule a
# later batch's answer grants the earlier batches, a HEDGE copy goes only for the newest batch in flight, and
# CALL_DEADLINE resends only the newest.
# Usage: readnewest.py TRACE.jsonl[.xz] LOAD_START_MS SECONDS
import json, collections, sys, lzma, bisect, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from tracerows import load
path = sys.argv[1]; lo = int(sys.argv[2]); hi = lo + 1000 * int(sys.argv[3])
merged=load(path, learners=True)
calls=collections.defaultdict(dict); missed=collections.defaultdict(set); hed=0; sent=0
for who, rs in merged.items():
    for t,kind,peer,b,index,ms in rs:
        if peer!=0: continue
        if kind==2:
            a=t-ms; c=calls[who]; c[a]=max(c.get(a,0),t)
            if int(index)==2: missed[who].add(a)
        if kind==11 and lo<=t-ms-500<hi: hed+=1
        if kind==1 and lo<=t<hi: sent+=1
o=collections.Counter(); wm=[]; wn=[]
for who,c in calls.items():
    st=sorted(c); suf=[0]*len(st); m=float('inf')
    for i in range(len(st)-1,-1,-1): m=min(m,c[st[i]]); suf[i]=m
    resend_of={}
    for a in missed[who]:
        i=bisect.bisect_left(st,a+995); j=bisect.bisect_right(st,a+1110)
        if i<j: resend_of[a]=st[i]
    dropped=set()
    for n,a in enumerate(st):
        if not lo<=a<hi: continue
        nxt=st[n+1] if n+1<len(st) else float('inf')
        newest=lambda t: nxt>=t
        if c[a]-a>500:
            o['main_copy']+=1
            if not newest(a+500) or suf[n]<=a+500: o['copy_dropped']+=1
        if a in resend_of:
            o['main_resend']+=1
            if not newest(a+1000) or suf[n]<=a+1000:
                o['resend_dropped']+=1; dropped.add(resend_of[a])
        wm.append(c[a]-a); wn.append(suf[n]-a)
    for r in dropped:
        if c[r]-r>500: o['resend_copy_dropped']+=1
    o['calls']+=sum(1 for a in st if lo<=a<hi)
total=sent+hed; drop=o['copy_dropped']+o['resend_dropped']+o['resend_copy_dropped']
print('traced: IndexSent',sent,'hedges',hed,'total',total); print(dict(o))
q=lambda xs,p: sorted(xs)[int(p*(len(xs)-1))]
print('batch wait p50/p90/p99 main',[q(wm,p) for p in (.5,.9,.99)],'new',[q(wn,p) for p in (.5,.9,.99)])
print(f"duplicates main {o['main_copy']+o['main_resend']} (of which resends {o['main_resend']}); dropped {drop}; leader ReadIndex calls {total} -> {total-drop} ({100*drop/total:.1f}% fewer)")
