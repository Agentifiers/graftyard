#!/usr/bin/env python3
"""Medians of run.sh's /prof rows: ms per entry and per command by stage, for each N and grow, with each stage's unit.

usage: table.py out.jsonl
"""
import json
import statistics
import sys

# Not CPU of the entry: the sends' wall, and the probe reads, which are timed apart from the leader's work.
OUT = {"peer_send_wall", "pulls_from", "pulls_from_after", "lease_rows", "scan_one", "scan_256"}
PROBES = OUT - {"peer_send_wall"}
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
prof = [r for r in rows if "perEntryMs" in r]
noop = next((r["processCpuMsPerRequest"] for r in rows if "noop" in r), None)
stages = list(prof[0]["unit"])
for n in sorted({r["cmds"] for r in prof}, reverse=True):
    sets = {(g, p): [r for r in prof if r["cmds"] == n and r["grow"] == g and r["peers"] == p]
            for g, p in sorted({(r["grow"], r["peers"]) for r in prof if r["cmds"] == n})}
    cols = [k for k in sets if k[1] > 0]
    r0 = sets[cols[0]][0]
    print(f"\nN={n}: {r0['pushesPerEntry']:.0f} pushes an entry, {r0['entryBytes'] / 1e3:.0f} KB an entry, "
          f"{r0['stmtsPerEntry']:.0f} statements, {r0['peers']} peers, {r0['learnerCallsPerEntry']} learner calls and "
          f"{r0['entriesCallsPerEntry']} /entries pages an entry")
    head = " | ".join(f"grow={g}: ms/entry, ms/cmd" for g, _ in cols)
    print(f"| stage | unit | {head} |")
    print("|---|---|" + "---|" * len(cols))
    for s in stages:
        cells = []
        for k in cols:
            m = statistics.median(r["perEntryMs"][s] for r in sets[k])
            # A probe read is timed apart from the entry's work, over PROBE_REPS calls: its cell is ms per call.
            cells.append(f"{m / 10:.3f} a call" if s in PROBES else f"{m:.2f}, {m / n:.3f}")
        print(f"| {s} | {r0['unit'][s]} | " + " | ".join(cells) + " |")
    for k, rs in sets.items():
        own = statistics.median(sum(v for s, v in r["perEntryMs"].items() if s not in OUT) for r in rs)
        line = f"grow={k[0]} peers={k[1]}: stages without peer_send_wall and probes {own:.1f} ms/entry ({own / n:.3f} ms/cmd)"
        # workerd's process CPU covers the whole request, fixture building and the sink objects too, so it is an upper
        # bound, and only on runs with no untimed entries.
        if k[0] == 0:
            cpu = statistics.median(r["processCpuMs"] / r["entries"] for r in rs)
            line += f"; workerd process CPU {cpu:.1f} ms/entry ({cpu / n:.3f} ms/cmd)"
        print(f"{line}; at the end {rs[0]['openPrs']} open pull requests and {rs[0]['leases']} lease rows; {len(rs)} runs")
# The leader's render by kind (ms per command, medians), and what each kind answered.
for n in sorted({r["cmds"] for r in prof}, reverse=True):
    for g in sorted({r["grow"] for r in prof if r["cmds"] == n}):
        rs = [r for r in prof if r["cmds"] == n and r["grow"] == g and r["peers"] > 0 and "renderByKind" in r]
        if not rs:
            continue
        kinds = sorted(rs[0]["renderByKind"])
        cells = ", ".join(f"{k} {statistics.median(r['renderByKind'][k]['msPer'] for r in rs):.3f}" for k in kinds)
        print(f"N={n} grow={g}: render_ok ms per command by kind: {cells}; statuses {rs[0].get('replyStatus')}")
print(f"\nruntime floor: {noop} ms of workerd process CPU per /noop request")
