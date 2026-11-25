#!/usr/bin/env python3
# A17 (tests/m5/a17/row.sh): per write, each watch object's delay from the write's Cloudflare `Date`, and the orb
# sampler's old and new reads after it. Usage: python3 tests/m5/a17/analyze.py tests/m5/results/raw/a17/<tag>
import json,re,glob,datetime,sys
O=sys.argv[1]
puts=[]
for f in glob.glob(O+"/put*.log"):
  m=re.search(r"(v\d) sha256 (\w+) written: orb (\d+)\.\.(\d+) ms, answer Date (.+ GMT)",open(f).read())
  d=int(datetime.datetime.strptime(m.group(5),"%a, %d %b %Y %H:%M:%S GMT").replace(tzinfo=datetime.timezone.utc).timestamp()*1000)
  puts.append(dict(v=m.group(1),h=m.group(2)[:8],date=d))
puts.sort(key=lambda p:p["date"])
v0=re.search(r"v0 sha256 (\w+)",open(O+"/deploy.log").read()).group(1)[:8]
seq=[v0]+[p["h"] for p in puts]
last=sorted(glob.glob(O+"/log*.log"),key=lambda f:int((re.search(r"log-(\d+)",f) or [0,1])[1]))[-1]
print("watch log:",last.split("/")[-1])
out={}
for l in open(last).read().splitlines()[2:]:
  h,j=l.split(" ",1); j=json.loads(j)
  isos={e["isolate"] for e in j["log"]}
  out[h]=dict(isolates=sorted(isos),ticks=j["ticks"],delays=[round((e["at"]-p["date"])/1000,2) for p in puts for e in j["log"] if e["hash"][:8]==p["h"]])
  print(h,out[h])
rows=sorted((json.loads(l) for l in open(O+"/sample.jsonl") if l.startswith("{")),key=lambda r:r["at"])
print("sampler rows",len(rows),"isolates",len({r['isolate'] for r in rows}),"colos",sorted({r.get('colo') for r in rows}))
for i,p in enumerate(puts):
  end=puts[i+1]["date"] if i+1<len(puts) else 1<<62
  prev=seq[i]
  win=[r for r in rows if p["date"]<=r["at"]<end]
  old=[r for r in win if r["hash"][:8]==prev]
  new=[r for r in win if r["hash"][:8]==p["h"]]
  other=[r for r in win if r["hash"][:8] not in (prev,p["h"])]
  print(p["v"],"Date",datetime.datetime.fromtimestamp(p["date"]/1000,datetime.timezone.utc).strftime("%H:%M:%S"),
        "reads",len(win),"old",len(old),"last old +%.2fs"%((old[-1]["at"]-p["date"])/1000) if old else "last old none",
        "first new +%.2fs"%((new[0]["at"]-p["date"])/1000) if new else "", "other",len(other))
  # isolates that served the old value before this write and the new after it
  pre={r["isolate"] for r in rows if r["at"]<p["date"] and r["hash"][:8]==prev}
  both={r["isolate"] for r in new if r["isolate"] in pre}
  print("   isolates on both sides:",len(both))
