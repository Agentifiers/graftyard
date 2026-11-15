import json,os,sys,urllib.request
acct,tok=os.environ["CLOUDFLARE_ACCOUNT_ID"],os.environ["CLOUDFLARE_API_TOKEN"]
H={"authorization":f"Bearer {tok}","content-type":"application/json"}
a,b,like=sys.argv[1:4]
q="""query($a:String!,$s:Time!,$e:Time!,$n:String!){viewer{accounts(filter:{accountTag:$a}){
 p: durableObjectsPeriodicGroups(limit:10000, filter:{datetime_geq:$s, datetime_lt:$e, name_like:$n}){ max{memoryUsageBytes} sum{cpuTime} dimensions{ name datetime } } }}}"""
r=json.load(urllib.request.urlopen(urllib.request.Request("https://api.cloudflare.com/client/v4/graphql",data=json.dumps({"query":q,"variables":{"a":acct,"s":a,"e":b,"n":like}}).encode(),headers=H),timeout=90))
if r.get("errors"): print(json.dumps(r["errors"])[:400]); sys.exit(1)
for x in sorted(r["data"]["viewer"]["accounts"][0]["p"], key=lambda x:x["dimensions"]["datetime"]):
    print(x["dimensions"]["datetime"][11:19], x["dimensions"]["name"], round(x["max"]["memoryUsageBytes"]/2**20), "MiB", round(x["sum"]["cpuTime"]/1e6,1),"s cpu")
