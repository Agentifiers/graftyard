#!/usr/bin/env python3
"""Summarizes an F43 soak (tests/m1/wp8/f43-soak.sh): cycles and stragglers, pushes, and the peer-call capture.

    python3 tests/m1/wp8/f43-summary.py <soak dir>

Each `stats-<cycle>.jsonl` holds every member's `/stats` read with `reset=1` at the end of that cycle, so the per-peer
counts add up across cycles without double counting. Round times are the leaders' `roundMs` of each cycle.
"""
import collections
import glob
import json
import os
import sys

d = sys.argv[1]
cycles = [l.rstrip("\n").split("\t") for l in open(os.path.join(d, "cycles.tsv"))]
by_chaos = collections.Counter(c[2].split()[0] for c in cycles)
behind = [c for c in cycles if c[3] != "ok"]
print(f"cycles {len(cycles)}: {dict(by_chaos)}; cycles with a member behind its leader 60 s after the fault: {len(behind)}")
for c in behind:
    print("  behind:", "\t".join(c))

pushes = [l.rstrip("\n").split("\t") for l in open(os.path.join(d, "pushes.tsv"))]
ok = [p for p in pushes if p[4].split()[0] == "0"]
ms = sorted(int(p[4].split()[1]) for p in ok)
q = lambda xs, p: xs[min(len(xs) - 1, int(p * len(xs)))] if xs else None
print(f"pushes {len(pushes)}: ok {len(ok)}, failed {len(pushes) - len(ok)}; ok p50 {q(ms, .5)} ms, p99 {q(ms, .99)} ms, max {ms[-1] if ms else None} ms")

calls = collections.Counter()
failed = collections.Counter()
late_failed = collections.Counter()
late, late_max = 0, 0
msgs = collections.Counter()
rounds = []
slow = []
for f in sorted(glob.glob(os.path.join(d, "stats-*.jsonl")), key=lambda f: int(f.rsplit("-", 1)[1].split(".")[0])):
    cyc = int(f.rsplit("-", 1)[1].split(".")[0])
    for line in open(f):
        line = line.strip()
        if not line:
            continue
        m = json.loads(line)
        for r in m.get("roundMs") or []:
            rounds.append(r)
            if r > 5000:
                slow.append((cyc, m["chain"], m["site"], r))
        for to, p in (m.get("peers") or {}).items():
            key = f"{m['site']}>{to}"
            calls[key] += p["ok"] + sum(p["failed"].values())
            for k, n in p["failed"].items():
                failed[(key, k)] += n
            for k, n in p["lateFailed"].items():
                late_failed[(key, k)] += n
            late += p["late"]
            late_max = max(late_max, p["lateMsMax"])
            for e in p["errors"]:
                msgs[(e["kind"], e["msg"][:120])] += 1
total = sum(calls.values())
print(f"member-to-member calls {total}; late answers stepped {late} (slowest {late_max} ms)")
for k in sorted({k for _, k in failed}):
    n = sum(v for (_, kk), v in failed.items() if kk == k)
    print(f"  failed {k}: {n} ({100 * n / max(total, 1):.2f}% of calls)")
for k in sorted({k for _, k in late_failed}):
    n = sum(v for (_, kk), v in late_failed.items() if kk == k)
    print(f"  missed the deadline, then failed {k}: {n}")
print("  by peer pair:", ", ".join(f"{p} {k} {n}" for (p, k), n in sorted(failed.items())))
print("last error messages kept (kind, text, times seen in the kept windows):")
for (k, t), n in msgs.most_common(12):
    print(f"  {k}: {t} [{n}]")
rounds.sort()
print(f"leader rounds {len(rounds)}: p50 {q(rounds, .5)} ms, p99 {q(rounds, .99)} ms, max {rounds[-1] if rounds else None} ms; over 5 s: {len(slow)}")
for s in slow:
    print("  slow round: cycle %d %s site %d %d ms" % s)
