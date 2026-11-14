#!/usr/bin/env python3
"""Runs one push through stream-push's driver Worker (`driver.js`) and prints its answer with the edge's memory for
the push's window, then checks the branch with ls-remote through the edge.

    python3 tests/streampush/drive.py <branch> <MiB>

Needs tests/am1/lf13/env-m3f.sh sourced, /tmp/sp/drv.token (the driver's TOKEN), and CLOUDFLARE_* for the memory
read. No token reaches argv or the output.
"""
import base64, datetime, json, os, subprocess, sys, time, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
DRIVER = "https://graft-m3f-sp-driver.<workers-subdomain>.workers.dev/push"
EDGE = os.environ["GRAFT_EDGE"]
ADMIN = dict(l.split(" ", 1) for l in open(os.path.join(os.environ["M3_STATE"], "m3-tokens.txt")).read().split("\n")
             if l)["admin"].strip()
TOKEN = open("/tmp/sp/drv.token").read().strip()
now = lambda: datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def memory(t0, t1):
    q = ('{ viewer { accounts(filter:{accountTag:"%s"}) { workersInvocationsAdaptive(limit:50, filter:{scriptName:'
         '"graft-m3f-edge", datetime_geq:"%s", datetime_leq:"%s"}) { dimensions { status scriptVersion } sum { requests'
         ' } max { memoryUsageBytes wasmMemoryBytes } } } } }') % (os.environ["CLOUDFLARE_ACCOUNT_ID"], t0, t1)
    r = urllib.request.Request("https://api.cloudflare.com/client/v4/graphql", data=json.dumps({"query": q}).encode(),
                               headers={"authorization": f"Bearer {os.environ['CLOUDFLARE_API_TOKEN']}",
                                        "content-type": "application/json"})
    rows = json.load(urllib.request.urlopen(r, timeout=60))["data"]["viewer"]["accounts"][0]["workersInvocationsAdaptive"]
    return [{"version": x["dimensions"]["scriptVersion"][:8], "status": x["dimensions"]["status"],
             "requests": x["sum"]["requests"], "max_memory_mb": round(x["max"]["memoryUsageBytes"] / 1e6, 1),
             "max_wasm_mb": round(x["max"]["wasmMemoryBytes"] / 1e6, 1)} for x in rows]


def main():
    branch, mib = sys.argv[1], sys.argv[2]
    m = subprocess.run(["node", os.path.join(HERE, "packgen.mjs"), "manifest", branch, mib], check=True,
                       capture_output=True, text=True).stdout
    t0 = now()
    req = urllib.request.Request(DRIVER, data=m.encode(), method="POST", headers={
        "authorization": f"Bearer {TOKEN}", "content-type": "application/json", "user-agent": "graftyard-sp",
        "x-push-auth": "Basic " + base64.b64encode(f"x:{ADMIN}".encode()).decode()})
    try:
        out = json.load(urllib.request.urlopen(req, timeout=3600))
    except urllib.error.HTTPError as e:
        out = {"driver": e.code, "body": e.read().decode()[:500]}
    t1 = now()
    oid = json.loads(m)["oid"]
    ls = urllib.request.Request(f"{EDGE}/m3org/stream-push.git/info/refs?service=git-upload-pack",
                                headers={"authorization": "Basic " + base64.b64encode(f"x:{ADMIN}".encode()).decode(),
                                         "user-agent": "git/2.47.0"})
    refs = urllib.request.urlopen(ls, timeout=600).read().decode("latin-1")
    landed = f"{oid} refs/heads/{branch}" in refs
    time.sleep(90)  # Analytics lags the request by about a minute.
    print(json.dumps({"branch": branch, "mib": int(mib), "pack_bytes": json.loads(m)["bytes"], "from": t0, "to": t1,
                      "answer": out, "landed": landed, "edge": memory(t0, now())}, indent=1))


if __name__ == "__main__":
    main()
