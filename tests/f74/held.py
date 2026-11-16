#!/usr/bin/env python3
# F74, m41 (`tests/m0/results/m41-held.md`): what holds voter-to-voter /raft legs once fan-out duplicates are gone.
# From a tsave file with the inbound trace (LEARNERS=1), it prints, per voter: event-loop stalls (Lag, 8: the 100 ms
# ticker woke `ms` late, so [at - ms, at] had no turn), storage confirms (Confirm, 10: [at - ms, at]); then, for every
# voter-to-voter call whose legs (total - work) were 1 s or more, how much of the out leg overlapped a confirm or stall
# on the caller and of the back leg on the callee, against the same calls shifted 7 s as a chance control; legs with
# and without a caller confirm in flight at send; each missed call's likeliest holder against STEP_DOWN's own-stall
# exclusion; the routes that entered inside each 1 s stall; and a 15 s timeline of confirms, stalls, /suspect, /read.
# Out and back are skew-corrected per pair as calls.py does ((min out - min back) / 2).
#   python3 tests/f74/held.py tsave-TAG.jsonl --from ISO --to ISO
import json,collections,datetime,bisect,sys
args=sys.argv[1:]; opt={args[i]:args[i+1] for i in range(1,len(args)-1,2)}
iso=lambda s:int(datetime.datetime.fromisoformat(s.replace("Z","+00:00")).timestamp()*1000)
T0,T1=iso(opt["--from"]),iso(opt["--to"])
rows=collections.defaultdict(set)
for line in open(args[0]):
    r=json.loads(line)
    if 'rows' in r: rows[r.get('learner',r.get('site'))].update(tuple(x) for x in r['rows'])
V=(0,1,2)
stalls={s:sorted((x[0]-x[5],x[0]) for x in rows[s] if x[1]==8 and T0-10000<=x[0]<=T1+10000) for s in V}
for s in V:
    tot=sum(b-a for a,b in stalls[s] if T0<=b<=T1); print('site',s,'stalls',len(stalls[s]),'stalled ms in window',tot, f'{tot/3000:.1f}%')
def overlap(s,a,b):
    return sum(max(0,min(b,y)-max(a,x)) for x,y in stalls[s] if y>a and x<b)
sent,ent,left,ans={},{},{},{}
for addr,rs in rows.items():
    for at,k,p,b,i,ms in rs:
        if not b: continue
        if k==23: sent[(addr,b)]=(at,p)
        elif k==25: ent[(p,b)]=at
        elif k==24: left[(p,b)]=at
        elif k==21: ans[(addr,b)]=at
agg=collections.Counter(); n=0
for key,(at,to) in sent.items():
    c=key[0]
    if c in V and to in V and T0<=at<=T1 and key in ent and key in ans and key in left:
        total=ans[key]-at; work=left[key]-ent[key]; legs=total-work
        if legs<1000: continue
        n+=1
        o=ent[key]-at; bk=ans[key]-left[key]
        # stalled time inside the call on caller and callee
        cal=overlap(c,at,ans[key]); cee=overlap(to,at,ent[key])
        agg['legs']+=legs; agg['caller stalled during call']+=cal; agg['callee stalled during out']+=cee
        agg['explained']+=min(legs,max(cal,cee))
        if cal+cee==0: agg['calls with no stall']+=1
print('voter-voter calls with legs >= 1 s:',n,dict(agg))
# Storage confirms in flight (Confirm row: at = end, ms = duration) on the callee across the back leg, the out leg.
conf={s:sorted((x[0]-x[5],x[0]) for x in rows[s] if x[1]==10 and x[5]>=50) for s in V}
def cover(iv,a,b):
    # union length of intervals iv within [a,b]
    segs=sorted((max(a,x),min(b,y)) for x,y in iv if y>a and x<b)
    tot=0;cur=None
    for x,y in segs:
        if cur is None or x>cur[1]:
            if cur: tot+=cur[1]-cur[0]
            cur=[x,y]
        else: cur[1]=max(cur[1],y)
    if cur: tot+=cur[1]-cur[0]
    return tot
agg=collections.Counter()
for key,(at,to) in sent.items():
    c=key[0]
    if c in V and to in V and T0<=at<=T1 and key in ent and key in ans and key in left:
        if (ans[key]-at)-(left[key]-ent[key])<1000: continue
        bk=(left[key],ans[key]); o=(at,ent[key])
        agg['back raw']+=max(0,bk[1]-bk[0]); agg['out raw']+=max(0,o[1]-o[0])
        agg['back: callee confirm in flight']+=cover(conf[to],*bk)
        agg['back: caller stalled']+=cover(stalls[c],*bk)
        agg['out: callee confirm in flight']+=cover(conf[to],*o)
        agg['out: caller confirm in flight']+=cover(conf[c],*o)
        agg['out: callee stalled']+=cover(stalls[to],*o)
for k,v in agg.items(): print(k,v)
for s in V:
    tot=cover(conf[s],T0,T1); print('site',s,'confirm>=50ms in flight',tot,f'{tot/3000:.1f}% of window')
print('--- union: out leg held by caller confirm or caller stall; back leg by callee confirm or callee stall')
agg=collections.Counter(); per=[]
for key,(at,to) in sent.items():
    c=key[0]
    if c in V and to in V and T0<=at<=T1 and key in ent and key in ans and key in left:
        legs=(ans[key]-at)-(left[key]-ent[key])
        if legs<1000: continue
        held=cover(conf[c]+stalls[c],at,ent[key])+cover(conf[to]+stalls[to],left[key],ans[key])
        agg['legs']+=legs; agg['held']+=min(held,legs); per.append(min(held,legs)/legs)
print(dict(agg), f"{agg['held']/agg['legs']:.0%}", 'per-call share p50',sorted(per)[len(per)//2])
# Control: same windows shifted by +7 s (random alignment) to see the chance overlap
agg=collections.Counter()
for key,(at,to) in sent.items():
    c=key[0]
    if c in V and to in V and T0<=at<=T1-10000 and key in ent and key in ans and key in left:
        legs=(ans[key]-at)-(left[key]-ent[key])
        if legs<1000: continue
        d=7000
        held=cover(conf[c]+stalls[c],at+d,ent[key]+d)+cover(conf[to]+stalls[to],left[key]+d,ans[key]+d)
        agg['legs']+=legs; agg['held']+=min(held,legs)
print('control +7 s:',f"{agg['held']/agg['legs']:.0%}")
# Prompt calls (legs < 300 ms): what share of their legs overlaps confirms (should be low)
agg=collections.Counter()
for key,(at,to) in sent.items():
    c=key[0]
    if c in V and to in V and T0<=at<=T1 and key in ent and key in ans and key in left:
        legs=(ans[key]-at)-(left[key]-ent[key])
        if not (0<legs<300): continue
        agg['n']+=1; agg['any confirm at send']+=any(x<=at<y for x,y in conf[c])
print('prompt calls',dict(agg))
agg=collections.Counter()
for key,(at,to) in sent.items():
    c=key[0]
    if c in V and to in V and T0<=at<=T1 and key in ent and key in ans and key in left:
        legs=(ans[key]-at)-(left[key]-ent[key])
        if legs<1000: continue
        agg['n']+=1; agg['any confirm at send']+=any(x<=at<y for x,y in conf[c])
print('slow calls',dict(agg))
print('--- confirm duration vs transaction size (Flushed n just before) and appended bytes')
for s in V:
    rs=sorted(x for x in rows[s] if T0<=x[0]<=T1 and x[1] in (10,15,16,12))
    lastn=None; bins=collections.defaultdict(list); q=0
    for at,k,p,b,i,ms in rs:
        if k==16: lastn=i
        elif k==12: q+=ms
        elif k==10:
            key='big txn (>=100 stmts)' if (lastn or 0)>=100 else 'small'
            bins[key].append(ms); lastn=None
    for k2,v in bins.items():
        v.sort(); print(s,k2,len(v),'p50',v[len(v)//2],'p90',v[int(.9*len(v))],'p99',v[int(.99*len(v))])
print('--- what ran in the frozen clock of each stall (rows stamped in [start, start+5ms])')
R=['other','/raft','/read','/commit','/commit_many','/outcome','/outcome_by_request','/suspect','/campaign','/status','/stats','/trace','/ping']
for s in V:
    st=[(a,b) for a,b in stalls[s] if T0<=b<=T1 and b-a>=1000]
    kinds=collections.Counter(); routes=collections.Counter(); n=0
    idx=sorted(rows[s])
    ats=[x[0] for x in idx]
    for a,b in st:
        lo=bisect.bisect_left(ats,a); hi=bisect.bisect_right(ats,a+5)
        for x in idx[lo:hi]:
            kinds[x[1]]+=1
            if x[1]==25: routes[R[x[4]%64] if x[4]%64<len(R) else x[4]%64]+=1
        n+=1
    print('site',s,'stalls>=1s',n,'total',sum(b-a for a,b in st),'rows by kind',dict(kinds.most_common(8)),'entered by route',dict(routes))
    # arrivals per second overall vs route
    allr=collections.Counter(R[x[4]%64] for x in rows[s] if x[1]==25 and T0<=x[0]<=T1 and x[4]%64<len(R))
    print('   window arrivals by route /s', {k: round(v/300,1) for k,v in allr.most_common(7)})
print('--- output gate test: out leg vs the caller confirm in flight at send; back leg vs callee confirm in flight at left')
mins=collections.defaultdict(lambda:[1<<62,1<<62])
for key,(at,to) in sent.items():
    if key[0] in V and to in V and key in ent and key in ans and key in left:
        m=mins[(key[0],to)]; m[0]=min(m[0],ent[key]-at); m[1]=min(m[1],ans[key]-left[key])
skew={k:(v[0]-v[1])//2 for k,v in mins.items()}
print('pair skew',skew)
def pc(v,q): v=sorted(v); return v[min(len(v)-1,int(q*len(v)))] if v else None
for leg in ('out','back'):
    gated=[];free=[];resid=[]
    for key,(at,to) in sent.items():
        c=key[0]
        if not (c in V and to in V and T0<=at<=T1 and key in ent and key in ans and key in left): continue
        if leg=='out':
            t=at; who=c; L=ent[key]-at-skew[(c,to)]
        else:
            t=left[key]; who=to; L=ans[key]-left[key]+skew[(c,to)]
        ends=[y for x,y in conf[who] if x<=t<y]
        if ends:
            r=max(ends)-t; gated.append(L); resid.append(L-r)
        else: free.append(L)
    print(leg,'with a confirm in flight: n',len(gated),'leg p50/p90',pc(gated,.5),pc(gated,.9),'| leg minus confirm remaining p10/p50',pc(resid,.1),pc(resid,.5),'| none in flight: n',len(free),'p50/p90',pc(free,.5),pc(free,.9))
print('--- are long confirms loop stalls? share of each confirm >= 500 ms covered by a ticker stall on the same member')
for s in V:
    cs=[(a,b) for a,b in conf[s] if b-a>=500 and T0<=b<=T1]
    tot=sum(b-a for a,b in cs); cov=sum(cover(stalls[s],a,b) for a,b in cs)
    print(s,'confirms>=500ms',len(cs),'ms',tot,'covered by stall',cov,f'{cov/max(tot,1):.0%}')
    st=[(a,b) for a,b in stalls[s] if T0<=b<=T1]
    print('   stalls',len(st),'ms',sum(b-a for a,b in st),'covered by a confirm',sum(cover(conf[s],a,b) for a,b in st))
print('--- timeline per 15 s: per site confirm p99 / stall ms / suspect, read arrivals')
for b in range(0,20):
    a0=T0+b*15000; a1=a0+15000; line=[]
    for s in V:
        cms=sorted(x[5] for x in rows[s] if x[1]==10 and a0<=x[0]<a1)
        st=cover(stalls[s],a0,a1)
        e=[x[4]%64 for x in rows[s] if x[1]==25 and a0<=x[0]<a1]
        line.append(f"s{s} cf{pc(cms,.99)} st{st} su{e.count(7)} rd{e.count(2)}")
    print(datetime.datetime.fromtimestamp(a0/1000,datetime.UTC).strftime('%H:%M:%S'),' | '.join(line))
print('--- voter->voter calls that missed 1 s (or unanswered): what held them')
cls=collections.Counter()
for key,(at,to) in sent.items():
    c=key[0]
    if not (c in V and to in V and T0<=at<=T1): continue
    a=ans.get(key); dl=at+1000
    if a is not None and a-at<=1000: continue
    end=a if a is not None else dl+5000
    excused=any(x<=at and y>dl for x,y in conf[c])
    if excused: cls['excused: own confirm began before send, open at deadline']+=1; continue
    own_c=cover(conf[c],at,min(end,ent.get(key,end)))
    own_s=cover(stalls[c],at,end)
    cee=0
    if key in left: cee=cover(conf[to]+stalls[to],ent.get(key,left[key]),end)
    elif key in ent: cee=cover(conf[to]+stalls[to],ent[key],end)
    if own_c>=300: cls['caller confirm began after send held the out leg (not excused)']+=1
    elif own_s>=300: cls['caller loop stall (not excused)']+=1
    elif cee>=300: cls['callee confirm or stall']+=1
    elif key not in ent: cls['never entered, no stall/confirm seen']+=1
    else: cls['none seen']+=1
for k,v in cls.most_common(): print(f'{v:6} {k}')
print('--- gate test: for out legs >= 1 s, entered minus the end of the caller confirm that was in flight latest before entry')
d=[];ctrl=[]
for key,(at,to) in sent.items():
    c=key[0]
    if not (c in V and to in V and T0<=at<=T1 and key in ent): continue
    e=ent[key]-skew[(c,to)]
    if e-at<1000: continue
    ends=[y for x,y in conf[c] if x<e and y>at and y<=e+50]
    if ends: d.append(e-max(ends))
    # control: random point
    ends2=[y for x,y in conf[c] if x<e+7000 and y>at+7000 and y<=e+7050]
    if ends2: ctrl.append(e+7000-max(ends2))
H=lambda v: collections.Counter(min(x//50,10)*50 for x in v)
print('n',len(d),'hist(ms after confirm end, 50 ms bins, 500=500+)',sorted(H(d).items()))
print('control n',len(ctrl),sorted(H(ctrl).items()))
print('--- per 5 s bucket: callee inbound (non-raft) rate vs median out leg into it; caller inbound vs median back leg')
inb={s:collections.Counter((x[0]-T0)//5000 for x in rows[s] if x[1]==25 and T0<=x[0]<=T1 and x[4]%64!=1) for s in V}
outs=collections.defaultdict(list); backs=collections.defaultdict(list)
for key,(at,to) in sent.items():
    c=key[0]
    if not (c in V and to in V and T0<=at<=T1 and key in ent and key in ans and key in left): continue
    outs[(to,(at-T0)//5000)].append(ent[key]-at-skew[(c,to)])
    backs[(c,(at-T0)//5000)].append(ans[key]-left[key]+skew[(c,to)])
import statistics
for name,d in (('out by callee load',outs),('back by caller load',backs)):
    bins=collections.defaultdict(list)
    for (s,b),v in d.items():
        rate=inb[s][b]/5
        bins[min(int(rate//100)*100,700)].append(statistics.median(v))
    print(name,{k:(len(v),int(statistics.median(v))) for k,v in sorted(bins.items())})
print('--- per 5 s per voter: inbound req/s vs confirm p90 and count')
bins=collections.defaultdict(list)
for s in V:
    for b in range(60):
        a0=T0+b*5000;a1=a0+5000
        n=sum(1 for x in rows[s] if x[1]==25 and a0<=x[0]<a1)/5
        cm=sorted(x[5] for x in rows[s] if x[1]==10 and a0<=x[0]<a1)
        if cm: bins[min(int(n//100)*100,800)].append((pc(cm,.9),len(cm)))
for k,v in sorted(bins.items()):
    print(k,'req/s: buckets',len(v),'confirm p90 median',sorted(x[0] for x in v)[len(v)//2],'confirms per 5 s median',sorted(x[1] for x in v)[len(v)//2])
print('--- first 30 s on site 0 (leader): confirms with the statements flushed before them')
rs=sorted(x for x in rows[0] if T0-20000<=x[0]<=T0+30000 and x[1] in (10,16))
print([(x[0]-T0, 'F' if x[1]==16 else 'C', x[4] if x[1]==16 else x[5]) for x in rs if (x[1]==10 and x[5]>=300) or (x[1]==16 and x[4]>=100)][:40])
print('--- before and after the read surge: legs, inbound, confirm p99 and stalled share per voter')
cut=int(opt.get('--surge','0') or 0) or T0+120000
for name,(a0,a1) in (('before',(T0,cut)),('after',(cut,T1))):
    L=[(ans[k]-sent[k][0])-(left[k][0]-ent[k]) if False else (ans[k]-sent[k][0])-(left[k]-ent[k]) for k in sent if k[0] in V and sent[k][1] in V and a0<=sent[k][0]<a1 and k in ans and k in left and k in ent]
    miss=sum(1 for k in sent if k[0] in V and sent[k][1] in V and a0<=sent[k][0]<a1 and (k not in ans or ans[k]-sent[k][0]>1000))
    n=sum(1 for k in sent if k[0] in V and sent[k][1] in V and a0<=sent[k][0]<a1)
    per=[]
    for s in V:
        rate=sum(1 for x in rows[s] if x[1]==25 and a0<=x[0]<a1)/((a1-a0)/1000)
        cm=sorted(x[5] for x in rows[s] if x[1]==10 and a0<=x[0]<a1)
        per.append(f's{s} {rate:.0f} req/s confirm p99 {pc(cm,.99)} stalled {cover(stalls[s],a0,a1)/(a1-a0):.0%}')
    print(name, f'calls {n} missed {miss} ({miss/max(n,1):.0%}) legs p50/p90 {pc(L,.5)}/{pc(L,.9)} |', ' | '.join(per))
