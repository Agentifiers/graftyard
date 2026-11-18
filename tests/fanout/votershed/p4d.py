#!/usr/bin/env python3
"""(c1b) review evidence (tests/am1/results/c-design.md, "(c1b) under review"): m47 leader stalls, tickets and poll load.
Needs /tmp/shed/rows.pkl (prep.py); uv run -q --with numpy --with scipy."""
exec(open(__import__('os').path.join(__import__('os').path.dirname(__file__),'p2a.py')).read().split('for s in sorted(rows):')[0])
STEP={0:'other',1:'leader',2:'walk',3:'spread',4:'send',5:'ask'}
for s in sorted(rows):
    V=rows[s]
    secs=sorted({x[0]//1000 for x in V if x[1]==25 and T0<=x[0]<=T1 and leader(x[0])==s})
    if len(secs)<20: continue
    sb={x[3]:x[5] for x in V if x[1]==27}
    E=[x for x in V if x[1]==25 and T0<=x[0]<=T1 and leader(x[0])==s]
    n=len(secs); c=collections.Counter((route(x[4]%64),STEP.get(sb.get(x[4]//64),'-')) for x in E)
    com=sum(v for (r,_),v in c.items() if r=='/commit')
    # commands decided: Applied index advance is entries, not commands; use /commit entries as the proxy
    ql=collections.Counter(x[4] for x in V if x[1]==26 and T0<=x[0]<=T1 and leader(x[0])==s)
    print(f'member {s}: led {n} s, {len(E)/n:.0f} req/s while leading; /commit {com/n:.0f}/s; requests per /commit {len(E)/max(1,com):.1f}')
    for (r,stp),v in c.most_common(12): print(f"     {r:22}{stp:7} {v/n:6.1f}/s")
