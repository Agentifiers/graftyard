#!/usr/bin/env python3
"""B0-02: are the leader's AppendEntries held behind its own storage confirm? (`tests/m0/results/carried-6.md`)

Usage: python3 tests/m0carried/held.py RUN.stats.json [...]

Reads one bench run's `/stats` (`BENCH_RAW=1`, test-faults trace kinds 10, 12, and 14, and each follower's `raftIn`).
For every AppendEntries with entries, it takes the leader's issue time (`AppendQueued`) and the follower's handler
start for the same body (`raftIn`, matched by size within 5 s), and removes the clock offset with the pair's median.
It bins each send by the longest leader confirm whose transaction began in the same turn (within 2 ms), and prints how
late the request reached the follower against the median. With nothing holding sends, lateness does not grow with the
same-turn confirm; held sends arrive later by about the confirm's length. (A per-send "held" count depends on the
clock baseline chosen, median or 5th percentile, so only the bins are reported.)
"""
import json
import sys


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(p * len(xs)))] if xs else None


BINS = [(0, 60), (60, 150), (150, 300), (300, 10**9)]


def run(path):
    d = json.load(open(path))
    lead = max((s for s in d if s["r"]), key=lambda s: len(s["r"]["commitMs"]))
    tr = lead["r"]["trace"]
    queued = {(t[2], t[3]): t for t in tr if t[1] == 12}
    confirms = [(t[0] - t[5], t[0], t[5]) for t in tr if t[1] == 10]
    rows = []
    for fol in (s for s in d if s["r"] and s["site"] != lead["site"]):
        by_bytes = {}
        for x in fol["r"].get("raftIn", []):
            by_bytes.setdefault(x[1], []).append(x)
        pairs = []
        for t in tr:
            if t[1] == 14 and t[2] == fol["site"] and (t[2], t[3]) in queued:
                q = queued[(t[2], t[3])]
                c = [x for x in by_bytes.get(q[5], []) if abs(x[0] - q[0]) < 5000]
                if len(c) == 1:
                    pairs.append((q[0], c[0][0]))
        if not pairs:
            continue
        base = pct([a - s for s, a in pairs], 0.5)
        for s, a in pairs:
            conf = max((c for b, _, c in confirms if abs(b - s) <= 2), default=0)
            rows.append((conf, a - s - base))
    out = {"run": path.split("/")[-1].split(".")[0], "sends": len(rows)}
    for lo, hi in BINS:
        g = [late for c, late in rows if lo <= c < hi]
        out[f"confirm {lo}-{hi if hi < 10**9 else ''}"] = {"n": len(g), "lateP50": pct(g, 0.5), "lateP90": pct(g, 0.9)}
    return out


if __name__ == "__main__":
    for p in sys.argv[1:]:
        print(json.dumps(run(p)))
