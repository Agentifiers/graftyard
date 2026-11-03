#!/usr/bin/env python3
"""Seed the BF-20 metadata tier on all three members alike, resumably.

seed.py TOTAL_ROWS [BATCH]. Progress per site lives in /home/user/bf20/seed.<site>; each line is "start n ms dbBytes digest".
Sites run in parallel threads; a failed call is retried (a reseed of the same keys is idempotent on the row set)."""
import json, os, subprocess, sys, threading, time

TOTAL = int(sys.argv[1])
BATCH = int(sys.argv[2]) if len(sys.argv) > 2 else 10_000
WRITTEN = int(os.environ.get("WRITTEN", "100"))
PFX = os.environ.get("PFX", "/home/user/bf20/seed.")


def done(site):
    p = f"{PFX}{site}"
    if not os.path.exists(p):
        return 0
    last = 0
    for line in open(p):
        s, n = line.split()[:2]
        last = max(last, int(s) + int(n))
    return last


def run(site):
    start = done(site)
    fails = 0
    while start < TOTAL:
        n = min(BATCH, TOTAL - start)
        body = json.dumps({"start": start, "n": n, "bytes": 1800, "written": WRITTEN})
        t = time.time()
        env = dict(os.environ, DL="55000", TO="70")
        out = subprocess.run(["/tmp/bf20/m.sh", str(site), "/bench_seed", body], capture_output=True, text=True, env=env).stdout
        ms = int((time.time() - t) * 1000)
        try:
            v = json.loads(out)["v"]
            assert "digest" in v, v
        except Exception as e:  # noqa: BLE001
            fails += 1
            print(f"site {site} start {start} failed ({fails}): {out[:200]!r} {e}", flush=True)
            time.sleep(min(60, 5 * fails))
            continue
        fails = 0
        with open(f"{PFX}{site}", "a") as f:
            f.write(f"{start} {n} {ms} {v['dbBytes']} {v['digest']}\n")
        print(f"site {site} {start + n}/{TOTAL} {ms} ms db {v['dbBytes'] / 1e9:.2f} GB", flush=True)
        start += n


ts = [threading.Thread(target=run, args=(s,)) for s in (0, 1, 2)]
[t.start() for t in ts]
[t.join() for t in ts]
print("seeded", TOTAL, flush=True)
