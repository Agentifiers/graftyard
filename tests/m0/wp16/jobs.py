#!/usr/bin/env python3
"""M0-WP16: turn the replay into InsightJobs for the container runner (run/wp16.ts).

  python3 wp16/jobs.py --events /home/user/wp16/events.json --truth /home/user/wp16/predict-r2.json \
      --out /home/user/wp16/jobs.json

Keeps the same fold as wp16/predict.py (live PathSet rows, the PathGlob overlap rule) and writes, per push: time, ref,
head, base, and every overlapping live ref with its head, shared-file count, push time, and the offline landed verdict
from predict-r2.json (the ground truth for the container's verdicts). Others are sorted by shared files, most first, then
most recent, which is the order the runner caps at PREDICT_PAIRS; the runner can also reorder by recency.
"""
import argparse, json

ap = argparse.ArgumentParser()
ap.add_argument("--events", required=True)
ap.add_argument("--truth", required=True)
ap.add_argument("--out", required=True)
a = ap.parse_args()


def overlap(p, q):
    if not p["truncated"] and not q["truncated"]:
        return len(p["set"] & q["set"])
    n = 0
    for x in p["paths"]:
        for y in q["paths"]:
            if x == y or (p["truncated"] and y.startswith(x + "/")) or (q["truncated"] and x.startswith(y + "/")):
                n += 1
    return n


events = json.load(open(a.events))["events"]
truth = {(j["i"], r): v for j in json.load(open(a.truth))["jobs"] for r, t, n, v in j["over"]}
merged = {e["ref"]: e["merged"] for e in events}
rows, jobs = {}, []
for e in events:
    e["set"] = set(e["paths"])
for e in events:
    for ref in [r for r in rows if merged.get(r, 0) <= e["t"]]:
        rows.pop(ref)
    others = []
    for r, row in rows.items():
        if r == e["ref"]:
            continue
        n = overlap(e, row)
        if n:
            others.append({"ref": r, "head": row["head"], "n": n, "t": row["t"], "truth": truth[(e["i"], r)]})
    others.sort(key=lambda o: (-o["n"], -o["t"]))
    jobs.append({"i": e["i"], "t": e["t"], "ref": e["ref"], "head": e["head"], "base": e["base"], "live": len(rows) - (e["ref"] in rows),
                 "others": others})
    rows[e["ref"]] = e
json.dump({"jobs": jobs}, open(a.out, "w"))
print(len(jobs), sum(len(j["others"]) for j in jobs), sum(1 for j in jobs for o in j["others"] if o["truth"] == "Conflict"))
