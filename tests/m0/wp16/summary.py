#!/usr/bin/env python3
"""M0-WP16 L0-05: score wp16/predict.py's offline run against its ground truth.

  python3 wp16/summary.py /home/user/wp16/predict-r2.json > results/raw/wp16-l0-05-offline.json

Reports, on the 1,000 sampled pairs: landed truth, raw tip-versus-tip verdicts, and how many non-overlapping pairs
conflict (the overlap rule's completeness). On all overlapping pairs: conflicts found at each PREDICT_PAIRS value under
two orders, most recent first and most shared files first, and the rank of each conflict in the size order.
"""
import json, sys
from collections import Counter

d = json.load(open(sys.argv[1]))
pairs, jobs = d["pairs"], d["jobs"]
over = [p for p in pairs if p["overlapFiles"]]
non = [p for p in pairs if not p["overlapFiles"]]
out = {
    "sample": {
        "pairs": len(pairs), "overlapping": len(over), "nonOverlapping": len(non),
        "landed": dict(Counter(p["landed"] for p in pairs)),
        "nonOverlappingLandedConflicts": sum(p["landed"] == "Conflict" for p in non),
        "raw": dict(Counter(p["raw"] for p in pairs)),
        "rawConflictLandedClean": sum(p["raw"] == "Conflict" and p["landed"] == "Clean" for p in pairs),
        "rawCleanLandedConflict": sum(p["raw"] == "Clean" and p["landed"] == "Conflict" for p in pairs),
    },
    "allOverlapping": {"jobs": len(jobs), "jobsWithOverlap": sum(1 for j in jobs if j["over"]),
                       "pairs": sum(len(j["over"]) for j in jobs), "verdicts": dict(Counter(o[3] for j in jobs for o in j["over"])),
                       "maxPerJob": max(len(j["over"]) for j in jobs), "jobsOver8": sum(1 for j in jobs if len(j["over"]) > 8)},
    "caps": {},
}
ranks = []
for j in jobs:
    by_size = sorted(j["over"], key=lambda o: (-o[2], -o[1]))
    ranks += [k for k, o in enumerate(by_size) if o[3] == "Conflict"]
out["sizeOrderConflictRanks"] = dict(sorted(Counter(ranks).items()))
for cap in (4, 8, 16, 32):
    row = {}
    for name, key in (("recent", lambda o: -o[1]), ("size", lambda o: (-o[2], -o[1]))):
        found = sum(1 for j in jobs for o in sorted(j["over"], key=key)[:cap] if o[3] == "Conflict")
        row[name] = {"found": found, "pairsRun": sum(min(cap, len(j["over"])) for j in jobs)}
    out["caps"][cap] = row
out["caps"]["truthConflicts"] = out["allOverlapping"]["verdicts"].get("Conflict", 0)
json.dump(out, sys.stdout, indent=1)
