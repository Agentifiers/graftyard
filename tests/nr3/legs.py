#!/usr/bin/env python3
# narrow-reads-3: where a learner's read-index round trip goes, from a gate's raw trace (tests/nr3/raw/trace-TAG.json,
# written by split.mjs before teardown). Run from the worktree root: python3 tests/nr3/legs.py gate12
# Per learner read-index batch: learner IndexSent (1) -> leader IndexTaken (5) -> learner ResStepped (3, own call).
# Batch numbers exceed 2^53 and lost precision in the JSON trace, so a batch is matched by its rounded number plus time:
# the leader's IndexTaken from that peer with the same rounded number between send and answer (20 ms skew allowed).
import json,sys,bisect,collections
D=json.load(open(f"tests/nr3/raw/trace-{sys.argv[1]}.json"))
def pct(xs,q):
    xs=sorted(xs); return xs[min(len(xs)-1,int(q*len(xs)))] if xs else None
f=next(v for k,v in D.items() if k.startswith("f")); T0,T1=f[0][0],f[-1][0]  # the leader, whichever site
taken=collections.defaultdict(list)
for at,kind,peer,batch,index,ms in f:
    if kind==5: taken[(peer,batch)].append(at)
addr={f"l{i}":192+i for i in range(8)}
for s in sorted(addr):
    a=addr[s]; sent={}; inb=[];back=[];tot=[];miss=0
    for at,kind,peer,batch,index,ms in D[s]:
        if kind==1 and T0<=at<=T1: sent.setdefault(batch,[]).append(at)
        elif kind==3 and ms==0 and sent.get(batch):
            t0=sent[batch].pop(0)
            c=[t for t in taken.get((a,batch),[]) if t0-20<=t<=at+20]
            if not c: miss+=1; continue
            t=min(c,key=lambda t:abs(t-t0))
            inb.append(t-t0); back.append(at-t); tot.append(at-t0)
    print(s,"n",len(tot),"miss",miss,"to leader p50/p90/p99 %s/%s/%s"%(pct(inb,.5),pct(inb,.9),pct(inb,.99)),
          "back p50/p90/p99 %s/%s/%s"%(pct(back,.5),pct(back,.9),pct(back,.99)),"total p50/p99 %s/%s"%(pct(tot,.5),pct(tot,.99)))
# Is the back leg the leader's output gate? Leader confirm intervals [at-ms, at]; for each matched batch, the leader
# confirm running when it took the ReadIndex, and how long that confirm still had to run.
conf=sorted((at-ms,at) for at,kind,peer,batch,index,ms in f if kind==10)
CS=[c[0] for c in conf]
def left_at(t):
    j=bisect.bisect_right(CS,t); best=0
    for k in range(max(0,j-5),j):
        if conf[k][0]<=t<=conf[k][1]: best=max(best,conf[k][1]-t)
    return best
for s in ["l3","l6","l1","l4","l0","l7"]:
    a=addr[s]; sent={}; rows=[]
    for at,kind,peer,batch,index,ms in D[s]:
        if kind==1 and T0<=at<=T1: sent.setdefault(batch,[]).append(at)
        elif kind==3 and ms==0 and sent.get(batch):
            t0=sent[batch].pop(0)
            c=[t for t in taken.get((a,batch),[]) if t0-20<=t<=at+20]
            if c:
                t=min(c,key=lambda t:abs(t-t0)); rows.append((at-t,left_at(t)))
    gated=[b for b,l in rows if l>0]; free=[b for b,l in rows if l==0]
    ex=[b-l for b,l in rows if l>0]
    print(s,"behind a leader confirm %d%%"%(100*len(gated)/len(rows)),"back p50/p99 gated %s/%s free %s/%s"%(pct(gated,.5),pct(gated,.99),pct(free,.5),pct(free,.99)),"| back minus confirm left p50/p99 %s/%s"%(pct(ex,.5),pct(ex,.99)))
# The learner's own side: IndexSent (1) -> IndexEnded (2, the call's answer in hand, `ms` the call's time from issue)
# -> ResStepped (3, the answer stepped in a later turn). Same batch number, same member clock, so no skew.
print("learner side: sent->issued (queue before fetch) | call (issue->answer) | answer->stepped (turn)")
for s in sorted(addr):
    sent={}; ended={}; pre=[];call=[];turn=[]
    for at,kind,peer,batch,index,ms in D[s]:
        if not (T0<=at<=T1): continue
        if kind==1: sent.setdefault(batch,[]).append(at)
        elif kind==2 and sent.get(batch):
            t0=sent[batch][0]; pre.append(max(0,at-ms-t0)); call.append(ms); ended[batch]=at
        elif kind==3 and ms==0 and batch in ended:
            turn.append(at-ended.pop(batch))
            if sent.get(batch): sent[batch].pop(0)
    print(s,"pre p50/p99 %s/%s"%(pct(pre,.5),pct(pre,.99)),"call p50/p90/p99 %s/%s/%s"%(pct(call,.5),pct(call,.9),pct(call,.99)),"turn p50/p99/max %s/%s/%s"%(pct(turn,.5),pct(turn,.99),max(turn) if turn else None))
# The leader's clock moves only across I/O, so its IndexTaken -> ResSent (0 ms here) cannot show CPU or the output
# gate, which holds a response after the handler returns until the writes before it are durable. Test the gate: for
# each batch, the end of the first leader confirm still running or starting at or after IndexTaken; back leg minus that.
CE=sorted(conf, key=lambda c:c[1])
ends=[c[1] for c in CE]
print("leader Lag rows (ticker >= 20 ms late):", sum(1 for r in f if r[1]==8))
for s in ["l3","l6","l1","l4"]:
    a=addr[s]; sent={}; g=[];rest=[];backs=[]
    for at,kind,peer,batch,index,ms in D[s]:
        if kind==1 and T0<=at<=T1: sent.setdefault(batch,[]).append(at)
        elif kind==3 and ms==0 and sent.get(batch):
            t0=sent[batch].pop(0)
            c=[t for t in taken.get((a,batch),[]) if t0-20<=t<=at+20]
            if not c: continue
            t=min(c,key=lambda t:abs(t-t0)); b=at-t
            j=bisect.bisect_left(ends,t)
            gate=(ends[j]-t) if j<len(ends) and ends[j]-t<=b else 0
            g.append(gate); rest.append(b-gate); backs.append(b)
    big=[(x,y) for x,y,z in zip(g,rest,backs) if z>=400]
    print(s,"back p99 %s | gate wait p50/p99 %s/%s | back minus gate p50/p90/p99 %s/%s/%s | of backs >= 400 ms (%d): gate share %.0f%%"%(pct(backs,.99),pct(g,.5),pct(g,.99),pct(rest,.5),pct(rest,.9),pct(rest,.99),len(big),100*sum(x for x,_ in big)/max(1,sum(x+y for x,y in big))))
# Does the back leg carry entries? Leader AppendTaken (19) rows to that peer at the same ms as its ResSent (6).
for s in ["l3","l6","l1","l4"]:
    a=addr[s]
    took=collections.Counter(at for at,kind,peer,*_ in f if kind==19 and peer==a)
    sent={}; w=[];wo=[]
    resat=sorted(at for at,kind,peer,*_ in f if kind==6 and peer==a)
    for at,kind,peer,batch,index,ms in D[s]:
        if kind==1 and T0<=at<=T1: sent.setdefault(batch,[]).append(at)
        elif kind==3 and ms==0 and sent.get(batch):
            t0=sent[batch].pop(0)
            c=[t for t in taken.get((a,batch),[]) if t0-20<=t<=at+20]
            if not c: continue
            t=min(c,key=lambda t:abs(t-t0))
            j=bisect.bisect_left(resat,t); r=resat[j] if j<len(resat) else None
            if r is None: continue
            n=took.get(r,0); (w if n else wo).append(at-t)
    print(s,"back leg with entries n %d p50/p99 %s/%s | without n %d p50/p99 %s/%s"%(len(w),pct(w,.5),pct(w,.99),len(wo),pct(wo,.5),pct(wo,.99)))
# With the send timestamps (Trace 20 CallArrived, 21 AnswerArrived; trees from narrow-reads-3's wire split on): per
# learner, out = the leader's handler start minus the learner's send (wire plus the leader isolate's input queue),
# body = reading the call's body; back = the learner's arrival minus the leader's handler return (output gate plus wire).
# The two clocks are different isolates', so out and back each carry their skew, which cancels in their sum.
arr=collections.defaultdict(list)
for at,kind,peer,batch,index,ms in f:
    if kind==20: arr[peer].append((at,index,ms))
if any(arr.values()):
    print("wire split: out (handler start - sent) | body read | back (arrived - left) | out+back, p50/p90/p99")
    for s in sorted(addr):
        a=addr[s]; out=[x[0]-x[1] for x in arr.get(a,[])]; body=[x[2] for x in arr.get(a,[])]
        back=[at-index for at,kind,peer,batch,index,ms in D[s] if kind==21 and index]
        print(s,"%s/%s/%s | %s/%s | %s/%s/%s"%(pct(out,.5),pct(out,.9),pct(out,.99),pct(body,.5),pct(body,.99),pct(back,.5),pct(back,.9),pct(back,.99)))
# The commit path's AppendEntries legs, for LM-1 (the operator after site day 13): the leader's calls to each voter
# follower. out = the follower's handler start minus the leader's send (CallArrived 20 on the follower's trace `v<site>`,
# from split.mjs); back = the leader's arrival minus the follower's handler return (AnswerArrived 21 on the leader's).
vs=sorted(k for k in D if k.startswith("v"))
if any(r[1] in (20,21) for r in f):
    lead=int(next(k for k in D if k.startswith("f"))[1:])
    print("commit path wire split, leader -> voter: out p50/p90/p99 | body p50/p99 | back p50/p90/p99")
    for k in vs:
        site=int(k[1:])
        rows=[(at-index,ms) for at,kind,peer,batch,index,ms in D[k] if kind==20 and peer==lead and T0<=at<=T1]
        back=[at-index for at,kind,peer,batch,index,ms in f if kind==21 and peer==site and index]
        out=[r[0] for r in rows]; body=[r[1] for r in rows]
        print(k,"%s/%s/%s | %s/%s | %s/%s/%s"%(pct(out,.5),pct(out,.9),pct(out,.99),pct(body,.5),pct(body,.99),pct(back,.5),pct(back,.9),pct(back,.99)))
