#!/usr/bin/env python3
"""colos.py TSAVE FROM TO -> `suspect` reports stepped on the voters (Reported 28) by edge colo and verdict. A colo is
three ASCII letters in a u32; 0 is a site script (`Colo::UNKNOWN`), which the SUSPECT rule does not count."""
import json,collections,datetime,lzma,sys
P,F,T=sys.argv[1:4]
iso=lambda s:int(datetime.datetime.fromisoformat(s.replace("Z","+00:00")).timestamp()*1000)
T0,T1=iso(F),iso(T)
V=["leader","resumed","started","bootstrapping","promised","busy","won","lost","mayNotLead"]
c=collections.defaultdict(collections.Counter)
for line in (lzma.open if P.endswith('.xz') else open)(P,'rt'):
    d=json.loads(line)
    if 'rows' not in d or d.get('learner') is not None: continue
    for x in d['rows']:
        if x[1]==28 and T0<=x[0]<T1: c[x[4]][V[x[5]] if x[5]<9 else '?']+=1
for k,v in sorted(c.items(),key=lambda z:-sum(z[1].values())): print('  colo',k.to_bytes(4,'big').lstrip(b'\0').decode() if k else 'site',sum(v.values()),dict(v))
