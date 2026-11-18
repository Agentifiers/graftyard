#!/usr/bin/env python3
"""(c1b) review evidence (tests/am1/results/c-design.md, "(c1b) under review"): m47 leader stalls, tickets and poll load.
Needs /tmp/shed/rows.pkl (prep.py); uv run -q --with numpy --with scipy."""
exec(open(__import__('os').path.join(__import__('os').path.dirname(__file__),'p2d.py')).read().split('# Little')[0].split("d=collections.defaultdict(list)\nfor s in rows:")[0])
from scipy.stats import spearmanr
import numpy as np
for s in sorted(rows):
    V=rows[s]
    E=[x for x in ent[s] if T0<=x[0]<=T1 and leader(x[0])==s]
    if len(E)<5000: continue
    rate=collections.Counter(x[0]//1000 for x in E)
    stall=collections.Counter()
    for x in V:
        if x[1]==8 and T0<=x[0]<=T1 and leader(x[0])==s:
            a,b=x[0]-x[5],x[0]
            for sec in range(a//1000,b//1000+1): stall[sec]+=max(0,min(b,(sec+1)*1000)-max(a,sec*1000))
    out=collections.defaultdict(list)
    for x in V:
        if x[1]==23 and T0<=x[0]<=T1 and (s,x[3]) in raft_in and leader(x[0])==s and (s,x[2]) in skew:
            c,at=raft_in[(s,x[3])]; out[x[0]//1000].append(at-x[0]-skew[(s,x[2])])
    secs=sorted(rate)
    r=[rate[b] for b in secs]; st=[stall[b] for b in secs]
    print(f'member {s}: {len(secs)} s; spearman(requests entered, ticker-stall ms) same second {spearmanr(r,st)[0]:.2f}')
    print('    stall share of the second by requests entered that second:',' '.join(f'{lo}-{hi}: {np.mean([st[i] for i in range(len(r)) if lo<=r[i]<hi])/1000 if any(lo<=v<hi for v in r) else float("nan"):.0%} (n{sum(1 for v in r if lo<=v<hi)})' for lo,hi in ((0,300),(300,600),(600,900),(900,10**9))))
    bs=[b for b in secs if out.get(b)]
    for k in (0,1,2):
        pr=[sum(rate.get(b-j,0) for j in range(k+1))/(k+1) for b in bs]; ol=[np.median(out[b]) for b in bs]
        print(f'    spearman(mean requests/s over the {k+1} s ending at the send second, out leg) {spearmanr(pr,ol)[0]:.2f} (n {len(bs)})')
