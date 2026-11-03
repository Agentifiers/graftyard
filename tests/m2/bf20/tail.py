#!/usr/bin/env python3
"""tail.py SCRIPT OUT: wrangler tail of one site script, keeping a count of outcomes by entrypoint and every event
whose outcome is not ok, with exceptions, or with error logs. Restarts the tail when it ends."""
import json, subprocess, sys, time, collections

script, out = sys.argv[1], sys.argv[2]
counts = collections.Counter()
last = time.time()
while True:
    p = subprocess.Popen(
        ["npx", "--prefix", "/home/user/workspace/repo/tests/m0", "wrangler", "tail", script, "--format", "json"],
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, cwd="/home/user/workspace/repo")
    buf = ""
    for line in p.stdout:
        buf += line
        if line.rstrip() != "}":
            continue
        try:
            ev = json.loads(buf)
        except json.JSONDecodeError:
            continue
        buf = ""
        key = f"{ev.get('entrypoint')}:{ev.get('outcome')}"
        counts[key] += 1
        errs = [l for l in ev.get("logs", []) if l.get("level") == "error"]
        if ev.get("entrypoint") == "Member":
            with open(out + ".ev", "a") as f:
                f.write(json.dumps([ev.get("eventTimestamp"), (ev.get("durableObjectId") or "")[:16],
                                    ((ev.get("event") or {}).get("request") or {}).get("url", "alarm" if "scheduledTime" in str(ev.get("event")) else "?"),
                                    ev.get("outcome"), ev.get("wallTime"), ev.get("cpuTime")]) + "\n")
        if ev.get("outcome") != "ok" or ev.get("exceptions") or errs:
            with open(out, "a") as f:
                f.write(json.dumps({"t": ev.get("eventTimestamp"), "ep": ev.get("entrypoint"), "outcome": ev.get("outcome"),
                                    "do": ev.get("durableObjectId"), "exc": ev.get("exceptions"), "err": errs, "cpu": ev.get("cpuTime"),
                                    "wall": ev.get("wallTime"), "url": (ev.get("event") or {}).get("request", {}).get("url")}) + "\n")
        if time.time() - last > 30:
            last = time.time()
            with open(out + ".counts", "w") as f:
                json.dump({"at": int(last * 1000), "counts": counts}, f)
    p.wait()
    time.sleep(2)
