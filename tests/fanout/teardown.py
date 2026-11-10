#!/usr/bin/env python3
"""teardown.py FIRST LAST -> tears down every running graft-m2b git shard container of the networks FIRST..LAST
(`git#<site>#net:<repo>`) through the test edge's `/_bed/kill?repo=&shard=`, as `workers/site/deploy.mjs
--roll-shards` does. Shards are derived caches: the next request catches the network up again. Container instances
of the `durable_object` policy never sleep on their own, so an idle shard bills its 6 GiB and 12 GB until torn down.
Pool shards (`git#<site>#pool:<n>`) are shared between networks and left running, unless `--pools` is given: then
every running pool shard is torn down too, through `/_bed/kill_pool?site=&pool=`. The bed token is read from
BED_TOKEN_FILE and sent on curl's stdin, never on argv."""
import json, os, re, subprocess, sys, urllib.request

pools = "--pools" in sys.argv[1:]
first, last = (int(a) for a in [a for a in sys.argv[1:] if a != "--pools"][:2])
acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
edge = os.environ["GRAFT_EDGE"]
bed = open(os.environ["BED_TOKEN_FILE"]).read().strip()
H = {"authorization": f"Bearer {tok}"}
api = lambda p: json.load(urllib.request.urlopen(urllib.request.Request(
    f"https://api.cloudflare.com/client/v4/accounts/{acct}/containers/{p}", headers=H), timeout=60))["result"]
done = {}
for app in api("applications"):
    if not app["name"].startswith("graft-m2b-site-"):
        continue
    for i in api(f"applications/{app['id']}/instances?per_page=1000")["instances"]:
        if i["status"].get("state") != "running":
            continue
        m = re.match(r"^git#(\d+)#net:(\d+)$", i.get("name") or "")
        p = re.match(r"^git#(\d+)#pool:(\d+)$", i.get("name") or "")
        if m and first <= int(m[2]) <= last:
            url = f"{edge}/_bed/kill?repo={m[2]}&shard={m[1]}"
        elif p and pools:
            url = f"{edge}/_bed/kill_pool?site={p[1]}&pool={p[2]}"
        else:
            continue
        r = subprocess.run(["curl", "-s", "-m", "60", "-X", "POST", "-H", "@-", "-o", "/dev/null", "-w", "%{http_code}",
                            url], input=f"x-bed-token: {bed}\n",
                           capture_output=True, text=True)
        done[i["name"]] = r.stdout or f"fail({r.returncode})"
        print(i["name"], done[i["name"]], flush=True)
codes = {}
for v in done.values():
    codes[v] = codes.get(v, 0) + 1
print(json.dumps({"torn_down": len(done), "codes": codes}))
