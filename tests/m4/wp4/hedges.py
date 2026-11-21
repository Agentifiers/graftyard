#!/usr/bin/env python3
"""hedges.py FROM TO STEPS [OBJECT] -> every member request whose `x-graft-sent-by` step (value % 16; `graftyard_lookup::SentBy`)
is in STEPS, FROM..TO, on the three graft-m4a sites, from Workers Logs events, one JSON line each: start, end, wall,
site, object, sending isolate (value // 16), step, message, outcome, status. Each query window halves until it is under
the 500-event cap; the last line counts windows still at it. B0-31 m65r31 ran it with STEPS 6 (`SentBy::Hedge`)."""
import json, os, sys, urllib.request
from datetime import datetime
acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
ms=lambda s:int(datetime.fromisoformat(s.replace("Z","+00:00")).timestamp()*1000)
F,T=ms(sys.argv[1]),ms(sys.argv[2]); STEPS={int(x) for x in sys.argv[3].split(",")}; OBJ=sys.argv[4] if len(sys.argv)>4 else None
H="$workers.event.request.headers.x-graft-sent-by"
def q(svc, frm, to):
    flt=[{"key":"$metadata.service","operation":"eq","type":"string","value":svc},{"key":H,"operation":"exists","type":"string"}]
    if OBJ: flt.append({"key":"$workers.durableObjectId","operation":"includes","type":"string","value":OBJ})
    body={"queryId":"q","timeframe":{"from":frm,"to":to},"view":"events","limit":500,"parameters":{"filters":flt}}
    for _ in range(4):
        try:
            r=urllib.request.Request(f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",data=json.dumps(body).encode(),method="POST",headers={"authorization":f"Bearer {tok}","content-type":"application/json"})
            return json.load(urllib.request.urlopen(r,timeout=120))["result"]["events"]["events"]
        except Exception as e: err=e
    raise err
full=0
for site,svc in enumerate(("graft-m4a-site-a","graft-m4a-site-b","graft-m4a-site-c")):
    t=F
    while t<T:
        step=int(os.environ.get("STEP_MS","5000"))
        ev=q(svc,t,min(T,t+step))
        while len(ev)>=500 and step>500:
            step//=2; ev=q(svc,t,min(T,t+step))
        if len(ev)>=500: full+=1
        for e in ev:
            w=e["$workers"]; h=(w.get("event") or {}).get("request",{}).get("headers",{})
            v=int(h.get("x-graft-sent-by","0")); 
            if v%16 not in STEPS: continue
            end=e["timestamp"]; wall=w.get("wallTimeMs") or 0
            print(json.dumps({"start":end-wall,"end":end,"wall":wall,"site":site,"obj":w.get("durableObjectId",""),"iso":v//16,"step":v%16,
                              "msg":e["$metadata"].get("message","")[:40],"outcome":w.get("outcome"),"status":(w.get("event") or {}).get("response",{}).get("status")}))
        t+=step
print(json.dumps({"chunks_at_cap":full}))
