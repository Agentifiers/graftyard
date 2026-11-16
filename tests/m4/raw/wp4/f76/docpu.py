import json,os,sys,urllib.request,collections
acct,tok=os.environ["CLOUDFLARE_ACCOUNT_ID"],os.environ["CLOUDFLARE_API_TOKEN"]
H={"authorization":f"Bearer {tok}","content-type":"application/json"}
a,b,like=sys.argv[1:4]
q="""query($a:String!,$s:Time!,$e:Time!,$n:String!){viewer{accounts(filter:{accountTag:$a}){
 p: durableObjectsPeriodicGroups(limit:10000, filter:{datetime_geq:$s, datetime_lt:$e, name_like:$n}){ sum{cpuTime activeTime} max{memoryUsageBytes} dimensions{ name datetimeMinute } } }}}"""
r=json.load(urllib.request.urlopen(urllib.request.Request("https://api.cloudflare.com/client/v4/graphql",data=json.dumps({"query":q,"variables":{"a":acct,"s":a,"e":b,"n":like}}).encode(),headers=H),timeout=90))
if r.get("errors"): print(json.dumps(r["errors"])[:500]); sys.exit(1)
by=collections.defaultdict(dict)
for x in r["data"]["viewer"]["accounts"][0]["p"]:
    by[x["dimensions"]["name"]][x["dimensions"]["datetimeMinute"][11:16]]=(round(x["sum"]["cpuTime"]/1e6,1), round(x["max"]["memoryUsageBytes"]/2**20))
for k,v in sorted(by.items()):
    print(k, " ".join(f"{m}:{c}s/{mb}M" for m,(c,mb) in sorted(v.items())))
