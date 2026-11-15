#!/usr/bin/env python3
"""teardown.py FIRST LAST [FIRST LAST ...] | --all-nets -> tears down every running graft-m2b (or $INSTALL) git shard container of
the networks in any range FIRST..LAST (`git#<site>#net:<repo>`), or of every network with `--all-nets` (older
installs' networks, such as 65536-65540 and 131072-131073, sit outside every range `rows.sh` uses), through the test
edge's `/_bed/kill?repo=&shard=`, as `workers/site/deploy.mjs --roll-shards` does. Shards are derived caches: the next request catches the network up again. Container instances
of the `durable_object` policy never sleep on their own, so an idle shard bills its 6 GiB and 12 GB until torn down.
Pool shards (`git#<site>#pool:<n>`) are shared between networks and left running, unless `--pools` is given: then
every running pool shard is torn down too, through `/_bed/kill_pool?site=&pool=`. The bed token is read from
BED_TOKEN_FILE and sent on curl's stdin, never on argv.

`--sweep` (no ranges): every network and pool shard, then a recheck of the install's running containers; exits 2 and
names each one still running after a minute. rows.sh and smoke.sh end every run with it."""
import json, os, re, subprocess, sys, urllib.request

sweep = "--sweep" in sys.argv[1:]
pools = "--pools" in sys.argv[1:] or sweep
nums = [int(a) for a in sys.argv[1:] if not a.startswith("--")]
if len(nums) % 2 or (not nums and "--all-nets" not in sys.argv[1:] and not sweep):
    sys.exit(__doc__)
ranges = [(0, 2**63)] if "--all-nets" in sys.argv[1:] or sweep else list(zip(nums[::2], nums[1::2]))
acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
edge = os.environ["GRAFT_EDGE"]
bed = open(os.environ["BED_TOKEN_FILE"]).read().strip()
H = {"authorization": f"Bearer {tok}"}
api = lambda p: json.load(urllib.request.urlopen(urllib.request.Request(
    f"https://api.cloudflare.com/client/v4/accounts/{acct}/containers/{p}", headers=H), timeout=60))["result"]
done = {}
if sweep:
    # A site fault left armed (L0-12's stall killed mid-run, m3fr2: `site=255` for 30 minutes) fails every member
    # call, so `/_bed/kill` cannot read the repository's record and every teardown answers 500. Clear it first.
    r = subprocess.run(["curl", "-s", "-m", "60", "-H", "@-", "-o", "/dev/null", "-w", "%{http_code}",
                        f"{edge}/_bed/fault?off=1"], input=f"x-bed-token: {bed}\n", capture_output=True, text=True)
    print(f"site fault off: {r.stdout or f'fail({r.returncode})'}", flush=True)
for app in api("applications"):
    if not app["name"].startswith(os.environ.get("INSTALL", "graft-m2b") + "-site-"):
        continue
    for i in api(f"applications/{app['id']}/instances?per_page=1000")["instances"]:
        if i["status"].get("state") != "running":
            continue
        m = re.match(r"^git#(\d+)#net:(\d+)$", i.get("name") or "")
        p = re.match(r"^git#(\d+)#pool:(\d+)$", i.get("name") or "")
        if m and any(a <= int(m[2]) <= b for a, b in ranges):
            url = f"{edge}/_bed/kill?repo={m[2]}&shard={m[1]}"
        elif p and pools:
            url = f"{edge}/_bed/kill_pool?site={p[1]}&pool={p[2]}"
        elif sweep:
            # A running container of no network or pool name: report it; the sweep's recheck fails on it.
            print(f"running, not a network or pool shard: {i.get('name')}", flush=True)
            continue
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
# `--sweep` (the end of every run): after the teardown above, list the install's running shard containers again,
# whatever their name, and fail loudly if any is still running, so a network or pool the run did not know about
# cannot bill unseen (rows m22's B0-34 seed shards, $16; B0-30's 73).
if "--sweep" in sys.argv[1:]:
    import time
    left = []
    for attempt in range(6):
        time.sleep(10)
        left = [(app["name"], i.get("name")) for app in api("applications")
                if app["name"].startswith(os.environ.get("INSTALL", "graft-m2b") + "-site-")
                for i in api(f"applications/{app['id']}/instances?per_page=1000")["instances"]
                if i["status"].get("state") == "running"]
        if not left:
            break
    if left:
        print(f"SWEEP FAILED: {len(left)} shard containers still running: {left}", file=sys.stderr)
        sys.exit(2)
    print("sweep: no shard container running")
elif any(k != "200" for k in codes):
    # A non-200 teardown leaves its container running; callers print "torn down" only on exit 0.
    print(f"TEARDOWN FAILED: {codes}", file=sys.stderr)
    sys.exit(1)
