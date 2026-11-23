#!/usr/bin/env python3
"""The hosted leader's CPU by route, from Workers Logs (tests/fanout/raw/wl-bl, wl-ql), for tests/am1/results/b033-profile.md.

For each run, over 90 to 300 s of the load: invocations a second, CPU ms a second and mean CPU per call by route
(`/raft` split by caller: a learner's address is 192 or more), the CPU in invocations under 5 ms, and the leader's
CPU a second in 30 s buckets with `/commit`'s CPU per call, to show whether a call's cost grows through the run.
Run from the repository root.
"""
import collections
import json
import lzma
import sys

sys.path.insert(0, "tests/fanout/b033probes/host")
from common import RUNS, ms  # noqa: E402

# Agent commands cut a second (`cuts.py`), b033-percmd.md and b033-quarter.md.
CUT = {"pfA": 47.8, "blB": 118.3, "blA": 84.0, "blC2": 82.7, "qlA": 43.7, "qlB": 45.1, "qlC": 41.8}

for tag in sys.argv[1:] or CUT:
    rows = json.load(lzma.open(f"tests/fanout/raw/wl-{tag[:2]}/{tag}-f0.json.xz", "rt"))
    start = ms(RUNS[tag][0])
    lo, hi = start + 90_000, start + 300_000
    secs = (hi - lo) / 1000
    by = collections.defaultdict(lambda: [0, 0.0])
    small = [0, 0.0]
    total = 0.0
    buckets = collections.defaultdict(collections.Counter)
    for x in rows:
        p = x["path"] or "other"
        if p == "/raft":
            p = "/raft learner" if int(x.get("from") or 0) >= 192 else "/raft voter"
        c = x["cpu"] or 0
        b = int((x["ts"] - start) // 30_000)
        if 0 <= b <= 10:
            buckets[b]["cpu"] += c
            if p == "/commit":
                buckets[b]["commit"] += 1
                buckets[b]["commit_cpu"] += c
        if not lo <= x["ts"] < hi:
            continue
        by[p][0] += 1
        by[p][1] += c
        total += c
        if c < 5:
            small[0] += 1
            small[1] += c
    cut = CUT[tag]
    print(f"{tag}: {total / secs:.0f} CPU ms/s over {cut} commands cut/s = {total / secs / cut:.2f} ms per command; "
          f"invocations under 5 ms carry {small[1] / secs:.0f} ms/s ({100 * small[1] / total:.0f}%)")
    for p, (n, c) in sorted(by.items(), key=lambda kv: -kv[1][1])[:7]:
        print(f"  {p:22} {n / secs:6.1f}/s {c / secs:6.1f} ms/s {c / n:5.2f} ms a call {c / secs / cut:5.2f} ms per command")
    print("  30 s buckets from load start: CPU ms/s, /commit ms a call")
    print("  " + "  ".join(f"{b * 30}s {g['cpu'] / 30:.0f} {g['commit_cpu'] / max(g['commit'], 1):.2f}"
                           for b, g in sorted(buckets.items())))
