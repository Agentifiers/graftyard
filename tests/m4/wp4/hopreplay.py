#!/usr/bin/env python3
"""hopreplay.py TRACE.jsonl[.xz] DEPS_LOG.json LOAD_START_MS SECONDS LEADER_SITE -> what the install `Authz` read
(`authz:1`) costs a repository leader over the owner's `Authz` read on the same entry, and what keeping the install
account's placement record on the member would save (B0-31 m65r31, `tests/m4/results/m4-wp4.md`). It reuses
slotdeps.py's cycles and slot model, then cuts each cycle's apply-to-append gap by what `authz:1` added over the entry's
other reads under each variant: the extra cut by the median hop, `authz:1` no slower than the owner's read, and
`authz:1` gone (the bound). A model, not the host's sample."""
import bisect, os, random, sys
here = os.path.dirname(os.path.abspath(__file__))
src = open(os.path.join(here, 'slotdeps.py')).read().split('rnd = random.Random(1)')[0]
import io, contextlib
with contextlib.redirect_stdout(io.StringIO()): exec(src)
def P(xs,p): xs=sorted(xs); return round(xs[min(len(xs)-1,int(p*len(xs)))]) if xs else None
diff=[r['authz:1']-r['authz:7100001'] for r in reads if 'authz:1' in r and 'authz:7100001' in r]
print('per entry authz:1 - owner authz: n',len(diff),'p10',P(diff,.1),'p50',P(diff,.5),'p90',P(diff,.9),'p99',P(diff,.99))
others=[max([v for k,v in r.items() if k!='authz:1'] or [0]) for r in reads if 'authz:1' in r]
print('entry other-reads max: p50',P(others,.5),'p99',P(others,.99), '; deps:all now p50', P([max(r.values()) for r in reads],.5),'p99',P([max(r.values()) for r in reads],.99))
def replay(newa):
    out=[]; saved=0
    for c in cycles:
        r=c[4]
        if 'authz:1' in r:
            o=max([v for k,v in r.items() if k!='authz:1'] or [0]); cur=max(r['authz:1'],o); new=max(newa(r),o)
            s=min(c[2], max(0,cur-new)); saved+=s; out.append((c[1], c[3]-s))
        else: out.append((c[1],c[3]))
    # slot model as /tmp/slot64f: entries back to back with these (round, cycle)
    t=0; app=[]; apl=[]
    for rnd_,cyc in out: app.append(t); apl.append(t+rnd_); t+=cyc
    rr=random.Random(2); sl=[]; q=[]
    for _ in range(20000):
        u=rr.uniform(0,t-5000); k=bisect.bisect_left(apl,u)+1
        if k>=len(app): continue
        sl.append(apl[k-1]-u); q.append(app[k]-u)
    return P(sl,.5),P(sl,.99),P(q,.99),round(t/1000,1),saved
print('as measured             slot p50/p99, queue p99, wall s, gap ms cut:', replay(lambda r: r['authz:1']))
print('record kept: = owner read', replay(lambda r: r.get('authz:7100001', r['authz:1'])))
med=P(diff,.5)
print(f'record kept: minus p50 hop {med}', replay(lambda r: max(0,r['authz:1']-med)))
print('authz:1 gone (bound)    ', replay(lambda r: 0))

# First order, on slotdeps' own slot model (cut-based, matches the host's slot p99 1,477 at 1,422): each command's
# slot and queue lose the gap savings of the cycles that end between its admission and its entry's append.
rnd = random.Random(1); samples, prev = [], t0 - 10_000
for c in sorted(rows[29]):
    taken = sum((c[3] >> (12 * i)) & 0xFFF for i in range(4))
    k = bisect.bisect_left(apps, c[0]) + ((c[2] >> 4) & 3)
    if k >= len(apps): break
    j = bisect.bisect_right(applies, apps[k]) - 1
    last = applies[j] if j >= 0 else apps[k]
    if t0 <= c[0] < t1:
        for _ in range(taken):
            adm = rnd.uniform(prev, c[0]); samples.append((adm, max(0, last - adm), apps[k] - adm, apps[k]))
    prev = c[0]
def first_order(newa):
    ends=[]; sav=[]
    for c in cycles:
        r=c[4]; s=0
        if 'authz:1' in r:
            o=max([v for k,v in r.items() if k!='authz:1'] or [0]); s=min(c[2], max(0,max(r['authz:1'],o)-max(newa(r),o)))
        ends.append(c[0]+c[3]); sav.append(s)
    cum=[0]
    for s in sav: cum.append(cum[-1]+s)
    sl=[]; q=[]
    for adm,slot,qq,ap in samples:
        a=bisect.bisect_right(ends,adm); b=bisect.bisect_right(ends,ap); d=cum[b]-cum[a]
        sl.append(max(0,slot-d)); q.append(max(0,qq-d))
    return P(sl,.5),P(sl,.99),P(q,.5),P(q,.99)
print('slotdeps model, first order: slot p50/p99, queue p50/p99')
print('  as measured              ', first_order(lambda r: r['authz:1']))
print(f'  record kept, -p50 hop {med}', first_order(lambda r: max(0,r['authz:1']-med)))
print('  record kept, = owner read', first_order(lambda r: r.get('authz:7100001', r['authz:1'])))
print('  authz:1 gone (bound)     ', first_order(lambda r: 0))
# What is left after the hop: every dep read as fast as it can be (the cycle a round alone, no gap at all), and
# the round's parts, from the row's split.
def no_gap():
    ends=[c[0]+c[3] for c in cycles]; sav=[c[2] for c in cycles]
    cum=[0]
    for x in sav: cum.append(cum[-1]+x)
    sl=[]; q=[]
    for adm,slot,qq,ap in samples:
        a=bisect.bisect_right(ends,adm); b=bisect.bisect_right(ends,ap); d=cum[b]-cum[a]
        sl.append(max(0,slot-d)); q.append(max(0,qq-d))
    return P(sl,.5),P(sl,.99),P(q,.5),P(q,.99)
print('  every gap gone (bound)   ', no_gap())
rr=[c[1] for c in cycles]
print('round (append to apply) p50/p90/p99', P(rr,.5), P(rr,.9), P(rr,.99), '; gap total ms', sum(c[2] for c in cycles), 'of wall', sum(c[3] for c in cycles))
