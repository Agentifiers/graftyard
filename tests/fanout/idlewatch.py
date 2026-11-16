#!/usr/bin/env python3
"""idlewatch.py MINUTES -> every 30 s for MINUTES, each graft-m2b (or $INSTALL) git shard container seen running,
once, as a JSON line with its name and start time, then a summary line. LM-1 round 34: after the site day that ships
promote-warm, no idle shard may start while nothing hosted runs (the reapers stay on, so a start is seen here before
reaper.sh tears it down within 60 s). Exit 1 if any started."""
import json, os, sys, time, urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
H = {"authorization": f"Bearer {tok}"}
api = lambda p: json.load(urllib.request.urlopen(urllib.request.Request(
    f"https://api.cloudflare.com/client/v4/accounts/{acct}/containers/{p}", headers=H), timeout=60))["result"]
end, seen = time.time() + 60 * float(sys.argv[1]), {}
while time.time() < end:
    try:
        for app in api("applications"):
            if not app["name"].startswith(os.environ.get("INSTALL", "graft-m2b") + "-site-"):
                continue
            for i in api(f"applications/{app['id']}/instances?per_page=1000")["instances"]:
                key = (i.get("name"), i.get("started_at"))
                if i["status"].get("state") == "running" and key not in seen:
                    seen[key] = time.strftime("%FT%TZ", time.gmtime())
                    print(json.dumps({"seen": seen[key], "app": app["name"], "name": key[0], "started": key[1]}), flush=True)
    except Exception as e:  # a failed listing is reported, not fatal
        print(json.dumps({"error": str(e)[:200]}), flush=True)
    time.sleep(30)
print(json.dumps({"minutes": float(sys.argv[1]), "started": len(seen)}))
sys.exit(1 if seen else 0)
