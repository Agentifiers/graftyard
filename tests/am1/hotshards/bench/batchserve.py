#!/usr/bin/env python3
"""batchserve.py DIR MODE [BATCH_BYTES] -> times ingesting every DIR/*.pack, in name order, into the container server
(containers/git/server, built to $SERVER, default /tmp/rbserver) on one CPU: MODE "one" is one POST /ingest per pack,
as a rehydrate's tail did before rehydrate-batch; "batch" is POST /ingest-batch with runs of up to BATCH_BYTES
(default 1 MiB, catchup::INGEST_BATCH_BYTES). Prints one JSON line: packs, calls, seconds, ms per pack, and the
repository's fsck. The server runs under taskset -c 0 with GRAFT_DATA in a fresh temporary directory."""
import glob, hashlib, json, os, subprocess, sys, tempfile, time, urllib.request

d, mode = sys.argv[1], sys.argv[2]
cap = int(sys.argv[3]) if len(sys.argv) > 3 else 1 << 20
data = tempfile.mkdtemp(prefix="rbdata-")
port = 8080
env = dict(os.environ, GRAFT_DATA=data)
srv = subprocess.Popen(["taskset", "-c", "0", os.environ.get("SERVER", "/tmp/rbserver")], env=env,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
url = f"http://127.0.0.1:{port}"
for _ in range(100):
    try:
        urllib.request.urlopen(url + "/health", timeout=1)
        break
    except Exception:
        time.sleep(0.1)


def post(path, body):
    r = urllib.request.urlopen(urllib.request.Request(url + path, data=body, method="POST"), timeout=600)
    return json.load(r)


post("/init?net=1", b"")
packs = [open(p, "rb").read() for p in sorted(glob.glob(f"{d}/*.pack"))]
t0, calls, later = time.time(), 0, 0
if mode == "one":
    for p in packs:
        post(f"/ingest?net=1&sha256={hashlib.sha256(p).hexdigest()}", p)
        calls += 1
else:
    i = 0
    while i < len(packs):
        body, n = [], 0
        while i < len(packs) and (not body or n + len(packs[i]) <= cap):
            body += [f"{hashlib.sha256(packs[i]).hexdigest()} {len(packs[i])}\n".encode(), packs[i]]
            n += len(packs[i])
            i += 1
        later += post("/ingest-batch?net=1", b"".join(body))["results"].count("later")
        calls += 1
s = time.time() - t0
repo = f"{data}/1.git"
tips = open(f"{d}/tips").read()
subprocess.run(["git", "-C", repo, "update-ref", "--stdin"], input="".join(f"create {l.split()[1]} {l.split()[0]}\n" for l in tips.splitlines()).encode(), check=True)
fsck = subprocess.run(["git", "-C", repo, "fsck", "--strict", "--no-dangling"], capture_output=True, text=True)
print(json.dumps({"dir": d, "mode": mode, "cap": cap, "packs": len(packs), "calls": calls, "later": later, "s": round(s, 1),
                  "ms_per_pack": round(1000 * s / len(packs), 2), "fsck": fsck.returncode, "fsck_out": (fsck.stdout + fsck.stderr)[:200]}))
srv.kill()
subprocess.run(["rm", "-rf", data])
