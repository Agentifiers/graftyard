#!/usr/bin/env python3
"""(c) design evidence (tests/am1/results/c-design.md): m47 leader stalls, releases and the round trip. Needs
/tmp/shed/rows.pkl (prep.py); uv run -q --with numpy --with scipy."""
exec(open(__import__('os').path.join(__import__('os').path.dirname(__file__),'p2d.py')).read().split('# Little')[0].split("d=collections.defaultdict(list)\nfor s in rows:")[0])
import lzma
from scipy.stats import spearmanr
tk=collections.defaultdict(list)
for l in lzma.open('/home/user/workspace/repo/tests/fanout/raw/memguard-m47b33.jsonl.xz','rt'):
    r=json.loads(l); t=iso(r['at'])
    for m in r['members']:
        if m['role'] and m['memSplit']: tk[m['site']].append((t,m['memSplit']['inFlight']['admitted']))
R=[]
for s in rows:
    T=sorted(tk[s]); tt=[a for a,_ in T]
    for x in rows[s]:
        if x[1]==23 and T0<=x[0]<=T1 and (s,x[3]) in raft_in and leader(x[0])==s and (s,x[3]) in ans and (s,x[3]) in left:
            i=B.bisect_right(tt,x[0])
            if not i or x[0]-tt[i-1]>3000: continue
            c,at=raft_in[(s,x[3])]; rt=ans[(s,x[3])]-x[0]-(left[(s,x[3])]-at)
            R.append((rt,T[i-1][1],open_at(s,x[0])))
print('sends with a memguard sample within 3 s before:',len(R))
print('spearman(round trip, open tickets)',round(spearmanr([r[0] for r in R],[r[1] for r in R])[0],2),' spearman(open, tickets)',round(spearmanr([r[2] for r in R],[r[1] for r in R])[0],2))
for lo,hi in ((0,100),(100,300),(300,600),(600,10**9)):
    v=[r[0] for r in R if lo<=r[1]<hi]; o=[r[2] for r in R if lo<=r[1]<hi]
    print(f'  open tickets {lo}-{hi}: n {len(v)} round trip p50 {pct(v,.5)} p90 {pct(v,.9)}; leader open p50 {pct(o,.5)}')
