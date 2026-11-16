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
end, seen, up, over = time.time() + 60 * float(sys.argv[1]), {}, {}, set()
now_s = lambda: time.strftime("%FT%TZ", time.gmtime())
while time.time() < end:
    try:
        running = set()
        for app in api("applications"):
            if not app["name"].startswith(os.environ.get("INSTALL", "graft-m2b") + "-site-"):
                continue
            for i in api(f"applications/{app['id']}/instances?per_page=1000")["instances"]:
                key = (i.get("name"), i.get("started_at"))
                if i["status"].get("state") != "running":
                    continue
                running.add(key)
                if key not in seen:
                    seen[key], up[key] = now_s(), time.time()
                    print(json.dumps({"seen": seen[key], "app": app["name"], "name": key[0], "started": key[1]}), flush=True)
        # A start that is gone is reported with how long it was seen running; one past 15 minutes once, as `over15`.
        for key in [k for k in up if k not in running]:
            print(json.dumps({"gone": now_s(), "name": key[0], "started": key[1], "ranS": round(time.time() - up.pop(key))}), flush=True)
        for key in [k for k in up if time.time() - up[k] > 900 and k not in over]:
            over.add(key)
            print(json.dumps({"over15": now_s(), "name": key[0], "started": key[1]}), flush=True)
    except Exception as e:  # a failed listing is reported, not fatal
        print(json.dumps({"error": str(e)[:200]}), flush=True)
    time.sleep(30)
for key, t in up.items():
    print(json.dumps({"stillRunning": now_s(), "name": key[0], "started": key[1], "ranS": round(time.time() - t)}), flush=True)
print(json.dumps({"minutes": float(sys.argv[1]), "started": len(seen)}))
sys.exit(1 if seen else 0)
