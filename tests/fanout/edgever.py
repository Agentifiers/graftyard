#!/usr/bin/env python3
"""edgever.py EDGE_LOG [SCRIPT] -> exit 0 only when SCRIPT (default graft-m2b-edge) serves, at 100%, the version the
site day's edge deploy wrote to EDGE_LOG (`tests/am1/runs/deploy-edge.sh`: "Current Version ID: ..."). Exit 3 when the
log has no version (the deploy did not finish, as m36's stopped while compiling, LM-1 round 24c) and 4 when the live
deployment is another version or a split. Account and token from the environment, the token only as a header."""
import json, os, re, sys, urllib.request

log, script = sys.argv[1], (sys.argv[2] if len(sys.argv) > 2 else os.environ.get("EDGE_SCRIPT", "graft-m2b-edge"))
text = open(log).read() if os.path.exists(log) else ""
m = re.findall(r"Current Version ID: ([0-9a-f-]{36})", text)
rev = text.splitlines()[1].strip() if len(text.splitlines()) > 1 else "?"
if not m:
    print(f"edge: {log} names no deployed version (rev {rev}); the site day's edge deploy did not finish")
    sys.exit(3)
want = m[-1]
acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
r = urllib.request.Request(f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/scripts/{script}/deployments",
                           headers={"authorization": f"Bearer {tok}"})
d = json.load(urllib.request.urlopen(r, timeout=60))["result"]["deployments"][0]
live = {v["version_id"]: v["percentage"] for v in d["versions"]}
if live != {want: 100}:
    print(f"edge: {script} serves {live} since {d['created_on']}, not {want[:8]} at 100% from {log} (rev {rev})")
    sys.exit(4)
print(f"edge: {script} serves {want[:8]} at 100% (rev {rev}), as {log} deployed")
