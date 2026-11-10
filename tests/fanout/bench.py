#!/usr/bin/env python3
"""Validates per second for one network in the git shard container, at 1, 2, 4 and 8 concurrent pushes.

  python3 tests/fanout/bench.py IMAGE [--cpus 1] [--secs 30] [--mode validate|push] [--conc 1,2,4,8]

Starts IMAGE with docker (--cpus, 6 GB, as a standard-2 instance), seeds network 1 with a 200-commit main through
/ingest and PUT /refs, makes one ~30 KB thin pack per push (a new branch off main adding one file), and posts them to
/validate with the tip and a detail line, as the site's shard does. Mode push also ingests the validated pack and
POSTs the branch to /refs after each validate, as the shard's catch-up does. Prints one JSON line per concurrency.
"""

import argparse
import base64
import hashlib
import json
import os
import random
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request


def git(cwd, *args, stdin=None):
    return subprocess.run(["git", *args], cwd=cwd, input=stdin, capture_output=True, check=True,
                          env={**os.environ, "GIT_AUTHOR_NAME": "b", "GIT_AUTHOR_EMAIL": "b@b",
                               "GIT_COMMITTER_NAME": "b", "GIT_COMMITTER_EMAIL": "b@b"}).stdout


def post(url, body=b"", method="POST"):
    req = urllib.request.Request(url, data=body, method=method)
    with urllib.request.urlopen(req, timeout=300) as r:
        return r.status, r.read()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("image")
    ap.add_argument("--cpus", default="1")
    ap.add_argument("--secs", type=float, default=30)
    ap.add_argument("--mode", default="validate")
    ap.add_argument("--conc", default="1,2,4,8")
    ap.add_argument("--packs", type=int, default=1000)
    a = ap.parse_args()

    wt = tempfile.mkdtemp(prefix="fanout-bench-")
    git(wt, "init", "-q", "-b", "main")
    # main: 200 commits over 50 files.
    imp = []
    mark = 0
    rnd = random.Random(1)
    for i in range(200):
        mark += 1
        data = (f"file {i}\n" + base64.b64encode(rnd.randbytes(600)).decode() + "\n").encode()
        parent = f"from :{mark - 1}\n" if i else ""
        imp.append(f"commit refs/heads/main\nmark :{mark}\ncommitter b <b@b> {1700000000 + i} +0000\n"
                   f"data 2\nc\n{parent}M 644 inline src/f{i % 50}.txt\ndata {len(data)}\n".encode() + data + b"\n")
    # One branch per push, each off main's tip with one ~40 KB file of its own.
    for j in range(a.packs):
        data = base64.b64encode(rnd.randbytes(30000)).decode().encode() + b"\n"
        imp.append(f"commit refs/heads/p{j}\ncommitter b <b@b> {1700001000 + j} +0000\ndata 2\np\nfrom :{mark}\n"
                   f"M 644 inline work/p{j}.txt\ndata {len(data)}\n".encode() + data + b"\n")
    git(wt, "fast-import", "--quiet", stdin=b"".join(imp))
    tip_main = git(wt, "rev-parse", "main").decode().strip()
    tips = git(wt, "for-each-ref", "--format=%(refname:short) %(objectname)", "refs/heads/p*").decode().split("\n")
    tips = [t.split() for t in tips if t]
    seed = git(wt, "pack-objects", "--stdout", "--revs", "-q", stdin=f"{tip_main}\n".encode())

    # Packed before the clock starts, so the client's own git does not count.
    packs = [git(wt, "pack-objects", "--stdout", "--revs", "--thin", "-q", stdin=f"{tip}\n^{tip_main}\n".encode())
             for _, tip in tips]

    name = f"fanout-bench-{os.getpid()}"
    port = 18000 + os.getpid() % 1000
    subprocess.run(["docker", "run", "-d", "--rm", "--name", name, f"--cpus={a.cpus}", "--memory=6g",
                    "-p", f"127.0.0.1:{port}:8080", a.image], check=True, capture_output=True)
    base = f"http://127.0.0.1:{port}"
    try:
        for _ in range(100):
            try:
                post(base + "/health", method="GET")
                break
            except Exception:
                time.sleep(0.1)
        post(base + "/init?net=1")
        post(base + f"/ingest?net=1&sha256={hashlib.sha256(seed).hexdigest()}", seed)
        post(base + "/refs?net=1", f"{tip_main} refs/heads/main\n".encode(), "PUT")

        nxt = [0]
        lock = threading.Lock()
        sizes = []
        for conc in [int(c) for c in a.conc.split(",")]:
            lat, errs = [], []
            stop = time.monotonic() + a.secs

            def worker():
                while time.monotonic() < stop:
                    with lock:
                        # Validate alone ingests nothing, so its packs cycle; push needs a new branch each time.
                        if a.mode == "push" and nxt[0] >= len(tips):
                            return
                        i = nxt[0] % len(tips)
                        nxt[0] += 1
                    ref, tip = tips[i]
                    p = packs[i]
                    head = f"{tip}\n".encode()
                    det = f"{tip} {'0' * 40} refs/heads/{ref}\n".encode()
                    t0 = time.monotonic()
                    try:
                        code, out = post(base + f"/validate?net=1&tipsn={len(head)}&detailn={len(det)}",
                                         head + det + p)
                        sum_ = json.loads(out)["checksum"]
                        if a.mode == "push":
                            post(base + f"/ingest?net=1&sha256={sum_}", b"")
                            post(base + "/refs?net=1", f"{tip} refs/heads/{ref}\n".encode())
                    except Exception as e:  # noqa: BLE001
                        errs.append(str(e)[:200])
                        continue
                    with lock:
                        lat.append(time.monotonic() - t0)
                        sizes.append(len(p))

            t0 = time.monotonic()
            ts = [threading.Thread(target=worker) for _ in range(conc)]
            for t in ts:
                t.start()
            for t in ts:
                t.join()
            wall = time.monotonic() - t0
            lat.sort()
            q = lambda f: round(lat[min(len(lat) - 1, int(f * len(lat)))] * 1000) if lat else None  # noqa: E731
            print(json.dumps({"image": a.image, "cpus": a.cpus, "mode": a.mode, "conc": conc, "n": len(lat),
                              "errors": len(errs), "err": errs[:2], "per_s": round(len(lat) / wall, 2),
                              "p50_ms": q(0.5), "p99_ms": q(0.99),
                              "pack_kb": round(sum(sizes) / max(1, len(sizes)) / 1024, 1)}), flush=True)
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True)
        subprocess.run(["rm", "-rf", wt])


if __name__ == "__main__":
    sys.exit(main())
