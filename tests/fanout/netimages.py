#!/usr/bin/env python3
"""netimages.py REPO [--none] [--image sha256:PREFIX] -> the graft-m2b (or $INSTALL) git shard containers of one
network (`git#<site>#net:REPO`) that are running, one JSON line each with its start and image digest, then a summary.
`--none`: exit 2 if any is running (B0-34's cold warm needs every one down). `--image`: exit 3 if a running one runs
another digest (round 22: site a's shard of repo:131296 still ran b63da3e6, which would not test connected-after)."""
import json, os, re, sys, urllib.request

args = sys.argv[1:]
if not args or args[0].startswith("--"):
    sys.exit(__doc__)
repo = args[0]
img = args[args.index("--image") + 1] if "--image" in args else None
acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
H = {"authorization": f"Bearer {tok}"}
api = lambda p: json.load(urllib.request.urlopen(urllib.request.Request(
    f"https://api.cloudflare.com/client/v4/accounts/{acct}/containers/{p}", headers=H), timeout=60))["result"]
running = []
for app in api("applications"):
    if not app["name"].startswith(os.environ.get("INSTALL", "graft-m2b") + "-site-"):
        continue
    for i in api(f"applications/{app['id']}/instances?per_page=1000")["instances"]:
        if i["status"].get("state") != "running" or not re.fullmatch(rf"git#\d+#net:{repo}", i.get("name") or ""):
            continue
        digest = (i.get("image") or "").rpartition("@")[2]
        running.append({"name": i["name"], "started": i.get("started_at"), "image": digest})
        print(json.dumps(running[-1]), flush=True)
other = [r for r in running if img and not r["image"].startswith(img)]
print(json.dumps({"net": repo, "running": len(running), "other_image": len(other)}))
sys.exit(2 if "--none" in args and running else 3 if other else 0)
