#!/usr/bin/env python3
"""B0-02's leader trace around each slow round (`tests/m0/results/carried-4.md`).

Usage: python3 tests/m0carried/stall.py RUN.stats.json [--over MS]

Reads one bench run's `/stats` (`BENCH_RAW=1`). For every AppendEntries with entries to the near follower whose call
took over MS (default 300), prints the leader's trace from 600 ms before it was queued to its end: inputs that wrote
(`Stepped`), deferred transactions (`Flushed`), storage confirms (`Confirm`), ticker lag (`Lag`), and each call's
queued, issued, and ended times, with the follower's `/raft` body arrival for the same round. Times are ms from the
slow call's queueing on the leader's clock; the follower's are on its own clock.
"""
import json
import sys

KIND = {4: "applied", 8: "lag", 10: "confirm", 12: "queued", 13: "issued", 14: "ended", 15: "stepped", 16: "flushed"}


def main(path, over):
    d = json.load(open(path))
    lead = next(s for s in d if s["r"] and s["r"]["role"] is True)
    near = min((s for s in d if s["r"] and s["r"]["role"] is False), key=lambda s: s["site"])
    tr = lead["r"]["trace"]
    issued = {(t[2], t[3]): t for t in tr if t[1] == 13}
    queued = {(t[2], t[3]): t for t in tr if t[1] == 12}
    raft_in = near["r"].get("raftIn", [])
    slow = [t for t in tr if t[1] == 14 and t[2] == near["site"] and t[5] > over]
    print(f"{path}: {len(slow)} calls to site {near['site']} over {over} ms")
    for e in slow:
        key = (e[2], e[3])
        q = queued.get(key)
        if not q:
            continue
        t0 = q[0]
        i = issued.get(key)
        print(f"\nround {e[3]} to {e[2]}: {q[5]} bytes, issued +{i[5] if i else '?'} ms, call {e[5]} ms")
        for t in tr:
            if t0 - 600 <= t[0] <= t0 + e[5] + 600 and t[1] in KIND and (t[1] not in (12, 13, 14) or t[2] == e[2]):
                print(f"  {t[0] - t0:+6d} {KIND[t[1]]:8s} peer {t[2]:3d} round {t[3]:6d} idx {t[4]:8d} ms {t[5]}")
        near_rows = [r for r in raft_in if abs(r[0] - t0) < 2000]
        print("  follower /raft near it (its clock, minus t0):", [(r[0] - t0, r[1], r[2], r[3]) for r in near_rows])


if __name__ == "__main__":
    over = int(sys.argv[sys.argv.index("--over") + 1]) if "--over" in sys.argv else 300
    main(sys.argv[1], over)
