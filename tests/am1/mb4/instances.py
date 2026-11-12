#!/usr/bin/env python3
"""merge-budget-4 item 3: each git shard container instance the Containers API lists for an install's sites, with the
configuration it ran on (vCPU, memory MiB, disk MB), so a run's instance size is read, not inferred from
SHARD_INSTANCE. Needs CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_API_TOKEN.

    INSTALL=graft-m3c python3 tests/am1/mb4/instances.py [started-after ISO]
"""
import json, os, sys, urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
install, after = os.environ.get("INSTALL", "graft-m3c"), (sys.argv[1] if len(sys.argv) > 1 else "")


def get(path):
    r = urllib.request.Request(f"https://api.cloudflare.com/client/v4/accounts/{acct}/containers/{path}",
                               headers={"authorization": f"Bearer {tok}"})
    return json.load(urllib.request.urlopen(r, timeout=60))["result"]


for app in get("applications"):
    if not (app["name"].startswith(f"{install}-site-") and app["name"].endswith("-gitshard")):
        continue
    for i in get(f"applications/{app['id']}/instances?per_page=1000")["instances"]:
        if i["started_at"] >= after:
            print(json.dumps({"app": app["name"], "name": i["name"], "started": i["started_at"],
                              "state": i["status"]["state"], "configuration": i["configuration"]}))
