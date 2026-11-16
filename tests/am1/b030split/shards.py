#!/usr/bin/env python3
"""shards.py [--keep NET,...] [--teardown] -> every running shard container of $INSTALL (default graft-m1), by site,
with its instance type and start time, from the Containers API. The end-of-run check: a run ends with only the
networks it named in --keep running (B0-30's own network, kept warm for the next run), and any other running shard
is listed as LEFT and exits 1. With --teardown the LEFT ones are torn down through tests/fanout/teardown.py's bed
route first. Account and token from the environment, by header only; the bed token from BED_TOKEN_FILE, on curl's
stdin only.

Why: after B0-30's first run, 73 idle standard-2 shards on graft-m1 billed $171 over 1.5 days. Its site script
(site day 8, `8753c0a`) lacked main's `3eb8315`, so every new leadership's warm `/catchup` counted as a client request
and moved the shard's `WARM_FOR` out another 24 h (b030-split.md, "Leaked containers")."""
import json, os, re, subprocess, sys, urllib.request

P = os.environ.get("INSTALL", "graft-m1")
keep = set()
if "--keep" in sys.argv:
    keep = {int(x) for x in sys.argv[sys.argv.index("--keep") + 1].split(",") if x}
acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
H = {"authorization": f"Bearer {tok}"}
g = lambda p: json.load(urllib.request.urlopen(urllib.request.Request(
    f"https://api.cloudflare.com/client/v4/accounts/{acct}/containers/{p}", headers=H), timeout=60))["result"]
left, kept = [], []
for app in g("applications"):
    if not app["name"].startswith(P + "-site-"):
        continue
    for i in g(f"applications/{app['id']}/instances?per_page=1000")["instances"]:
        if i["status"].get("state") != "running":
            continue
        name = i.get("name") or ""
        m = re.match(r"^git#(\d+)#net:(\d+)$", name)
        row = {"name": name, "vcpu": i["configuration"]["vcpu"], "started": i["started_at"][:19]}
        (kept if m and int(m[2]) in keep else left).append(row)
print(json.dumps({"install": P, "kept": kept, "left": left}))
if left and "--teardown" in sys.argv:
    nets = sorted({int(r["name"].split(":")[1]) for r in left if "#net:" in r["name"]})
    args = [x for n in nets for x in (str(n), str(n))] + (["--pools"] if any("#pool:" in r["name"] for r in left) else [])
    if not keep and any("#pool:" in r["name"] for r in left):
        args = ["--all-nets", "--pools"]
    if args:
        subprocess.run([sys.executable, "tests/fanout/teardown.py", *args], env={**os.environ, "INSTALL": P}, check=False)
sys.exit(1 if left else 0)
