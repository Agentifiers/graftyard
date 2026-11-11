#!/bin/bash
# Every 120 s, one line per sample of the graft-m2b shard containers running, by name (Containers API), for an idle
# window: no load, no reaper. One orb service; stop it when done.
#   bash tests/fanout/idlewatch.sh TAG   -> tests/fanout/raw/idlewatch-TAG.log
cd "$(dirname "$0")/../.."
out=tests/fanout/raw/idlewatch-$1.log
while true; do
  python3 - >> "$out" 2>&1 <<'EOF'
import json, os, time, urllib.request
acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
H = {"authorization": f"Bearer {tok}"}
api = lambda p: json.load(urllib.request.urlopen(urllib.request.Request(
    f"https://api.cloudflare.com/client/v4/accounts/{acct}/containers/{p}", headers=H), timeout=60))["result"]
run = []
for app in api("applications"):
    if app["name"].startswith("graft-m2b-site-"):
        run += [i.get("name") for i in api(f"applications/{app['id']}/instances?per_page=1000")["instances"]
                if i["status"].get("state") == "running"]
print(json.dumps({"at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "running": len(run), "names": sorted(run)}))
EOF
  sleep 120
done
