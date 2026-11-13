#!/usr/bin/env python3
"""B0-02: is the leader's event loop behind? (`tests/m0/results/carried-6.md`)

Usage: python3 tests/m0carried/clamp.py RUN.stats.json [...]

workerd clamps `Date.now()` to the earliest timeout deadline of the active scope, including a timer that fired and whose
callback has not run yet (`TimerChannel::now`, `io-channels.h`). The member's 100 ms ticker (`Host::kick`) is always
armed, so while the leader's event loop is behind, every event the trace stamps reads that overdue tick's deadline:
the trace piles onto one millisecond phase, confirms come out as exact multiples of 100 ms, and the ticker's own `Lag`
trace reads 0. Prints, per run, the share of the leader's trace at its most common ms phase (about 0.02 when the loop
keeps up), the confirms of 150 ms or more, and the run's commit p99.

It also bins every send by what was left of an earlier transaction's confirm when it was issued (`late` against a
local median, clock offset from the call's own two legs): a send issued while an earlier write is unconfirmed waits for
that confirm (https://developers.cloudflare.com/durable-objects/api/storage-api/, `allowUnconfirmed`).
"""
import bisect
import collections
import json
import sys


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(p * len(xs)))] if xs else None


def leader(path):
    d = json.load(open(path))
    return d, max((s for s in d if s["r"]), key=lambda s: len(s["r"]["commitMs"]))


def clamp(path):
    _, lead = leader(path)
    tr = lead["r"]["trace"]
    phase, n = collections.Counter(t[0] % 100 for t in tr).most_common(1)[0]
    slow = [t[5] for t in tr if t[1] == 10 and t[5] >= 150]
    p99 = json.load(open(path.replace(".stats.", ".bench."))).get("commitMs", {}).get("p99")
    return {"run": path.split("/")[-1].split(".")[0], "clampShare": round(n / len(tr), 2), "phase": phase,
            "slowConfirms": len(slow), "exact100": sum(1 for x in slow if x % 100 == 0), "commitP99": p99}


def earlier(paths):
    rows = []
    for path in paths:
        d, lead = leader(path)
        tr = lead["r"]["trace"]
        conf = [(t[0] - t[5], t[0]) for t in tr if t[1] == 10]
        q = {(t[2], t[3]): t for t in tr if t[1] == 12}
        for fol in (s for s in d if s["r"] and s["site"] != lead["site"]):
            by = {}
            for x in fol["r"].get("raftIn", []):
                by.setdefault(x[1], []).append(x)
            calls = []
            for t in tr:
                if t[1] == 14 and t[4] == 0 and t[2] == fol["site"] and (t[2], t[3]) in q:
                    s = q[(t[2], t[3])]
                    c = [x for x in by.get(s[5], []) if abs(x[0] - s[0]) < 5000]
                    if len(c) == 1:
                        calls.append((s[0], c[0][0], c[0][0] + c[0][2] + c[0][3], t[0]))
            calls.sort()
            ts = [c[0] for c in calls]
            offs = [((c[1] - c[0]) - (c[3] - c[2])) / 2 for c in calls]
            fwd = [c[1] - c[0] - offs[i] for i, c in enumerate(calls)]
            for i, (s0, fin, _, _) in enumerate(calls):
                lo, hi = bisect.bisect_left(ts, s0 - 3000), bisect.bisect_right(ts, s0 + 3000)
                left = max((e - s0 for b, e in conf if b < s0 - 2 and e > s0), default=0)
                same = max((e - b for b, e in conf if abs(b - s0) <= 2), default=0)
                if same < 60:
                    rows.append((left, fin - s0 - pct(offs[lo:hi], 0.5) - pct(fwd[lo:hi], 0.5)))
    out = {}
    for lo, hi in [(0, 1), (1, 50), (50, 150), (150, 10**9)]:
        g = [late for left, late in rows if lo <= left < hi]
        out[f"earlier tx left {lo}-{hi if hi < 10**9 else ''} ms"] = {"n": len(g), "lateP50": pct(g, 0.5), "lateP90": pct(g, 0.9)}
    return out


if __name__ == "__main__":
    for p in sys.argv[1:]:
        print(json.dumps(clamp(p)))
    print(json.dumps(earlier(sys.argv[1:])))
