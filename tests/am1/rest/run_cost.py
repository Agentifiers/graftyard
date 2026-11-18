#!/usr/bin/env python3
"""Marginal Cloudflare cost of each am1-rest hosted run on a test install, from `scripts/cost.py`'s prices
(`research/prices.json` through `cost_model.price`) and its per-operation figures (`ENTRY_ROWS`, `CPU_MS`, `DO_GB`,
`PUSH_VCPU_S`). Overage prices only: the install's included allowances are assumed already spent by earlier runs, so
each figure is an upper bound. The workload of each run is its row's rates, as `tests/am1/rest/a1.mjs --dry` prints
them. Run from the worktree root:

    python3 tests/am1/rest/run_cost.py
"""
import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("cf_cost", ROOT / "scripts" / "cost.py")
cf = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cf)
price = cf.price

MEMBERS = 3
EDGE_REQS_PER_CMD = 1        # one edge request per command or read the driver sends
MEMBER_REQS_PER_CMD = 4      # commit, outcome, and two AppendEntries a command, as B0-18's leader counts show [guess]
FEED_HELD_S = 25             # a held poll's wall time on a member (FEED_WAIT_MAX)


def run(name, seconds, cmds_per_s, reads_per_s, polls, pushes_per_s, pack_kb, r2_puts=0.0):
    entries = cmds_per_s * seconds
    rows = entries * MEMBERS * cf.ENTRY_ROWS
    edge_reqs = (cmds_per_s + reads_per_s) * seconds * EDGE_REQS_PER_CMD + polls * seconds / FEED_HELD_S
    do_reqs = cmds_per_s * seconds * MEMBER_REQS_PER_CMD + reads_per_s * seconds + polls * seconds / FEED_HELD_S
    cpu_ms = cmds_per_s * seconds * cf.CPU_MS["push"] + reads_per_s * seconds * cf.CPU_MS["api"]
    # Members are busy the whole run; held polls keep their objects alive too (duration bills 128 MB per object).
    do_gbs = (MEMBERS + 2) * seconds * cf.DO_GB
    vcpu_s = pushes_per_s * seconds * cf.PUSH_VCPU_S * 2          # validate on the home shard plus the warm copy
    r2_a = pushes_per_s * seconds * (2 if pack_kb > 64 else 0) + r2_puts
    usd = {
        "DO rows written": rows / 1e6 * price("cf.do.sqlite.rows_written"),
        "DO requests": do_reqs / 1e6 * price("cf.do.requests"),
        "DO duration": do_gbs / 1e6 * price("cf.do.duration"),
        "Workers requests": edge_reqs / 1e6 * price("cf.workers.requests"),
        "Workers CPU": cpu_ms / 1e6 * price("cf.workers.cpu"),
        "Containers vCPU": vcpu_s * price("cf.containers.vcpu"),
        "R2 class A": r2_a / 1e6 * price("cf.r2.standard.class_a"),
    }
    return name, seconds, sum(usd.values()), usd


# Rates from `a1.mjs --dry` (group commands/s, reads/s, pollers, agent pushes/s, mean pack KB).
RUNS = [
    run("B0-29, 600 s", 600, 27.6, 33 + 200, 500, 4.17, 88),
    run("B0-31, 600 s buckets up + 600 s placement failing", 1200, 27.6, 233, 500, 4.17, 1024),
    run("B0-32, 180 s with the burst", 180, 27.6 + 500 / 180, 233, 500, 4.17 + 500 / 180, 256),
    run("B0-33, 300 s at 5,000 agents", 300, 256.3, 33, 0, 41.7, 88),
    run("B0-34, 600 s with clones and a shard kill", 600, 27.6, 234, 500, 4.17, 88),
    run("B0-35, 600 s with 500 PR pollers", 600, 27.6, 233 + 100, 500, 4.17, 88),
    run("L0-12 A1 replay, 600 s, leases and kills", 600, 27.6, 233, 500, 4.17, 88),
    run("LF-13, 2 modes x 3 runs, about 3 min each", 6 * 180, 2, 5, 1000 / 6, 0, 0, r2_puts=200),
    run("LF-4, one pass", 60, 0.6, 1, 0, 0, 0),
    run("B0-36 and B0-37 reruns with the new Apps, 100 per kind", 3600, 1.5, 3, 0, 1.0, 4),
]

if __name__ == "__main__":
    print("| Run | Wall time | Upper-bound cost | Largest lines |")
    print("| --- | --- | --- | --- |")
    total = 0.0
    for name, s, usd, lines in RUNS:
        total += usd
        top = sorted(lines.items(), key=lambda kv: -kv[1])[:3]
        print(f"| {name} | {s / 60:.0f} min | ${usd:,.2f} | " + ", ".join(f"{k} ${v:,.2f}" for k, v in top) + " |")
    print(f"| **All runs once** | | **${total:,.2f}** | |")
