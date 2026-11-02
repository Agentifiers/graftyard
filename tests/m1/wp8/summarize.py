#!/usr/bin/env python3
"""Summarizes one BM-1 step's observer and stats files (tests/m1/wp8/bm1.sh writes them):

- leader moves: every change of a watched group's leader, timed on the members' own clocks (`now` in `/status`), and
  for each group the first sample after the fault starts that has a leader off the faulted site;
- commit latency: each member's `commitMs` samples (`/stats`), split at the fault's start when `stats-at-fault.jsonl`
  holds the samples before it (`fault.sh` with FAULT_STATS saves and resets them there);
- `STEP_DOWN` alarms and failed calls.

    python3 tests/m1/wp8/summarize.py <step dir> [fault site, default 0]
"""
import json
import pathlib
import re
import sys

step = pathlib.Path(sys.argv[1])
site = int(sys.argv[2]) if len(sys.argv) > 2 else 0


def fault_at():
    """The plan's `from` (Cloudflare's clock) from the fault log run.sh or clients-fault.sh wrote."""
    for f in [step / "fault.log", step / "run" / "fault.log"]:
        if f.exists():
            m = re.search(r'fault on \d+: \{"now":\d+,"plan":\{"from":(\d+)', f.read_text())
            if m:
                return int(m.group(1))
    return None


def pct(v, p):
    return v[min(len(v) - 1, len(v) * p // 100)] if v else None


start = fault_at()
print(f"fault on site {site} from {start}")
leaders, moved, changes = {}, {}, []
for line in (step / "observe.jsonl").read_text().splitlines():
    r = json.loads(line)
    for g in r["groups"]:
        ms = g["members"]
        now = max([m.get("now") or 0 for m in ms]) or r["at"]
        lead = [m["site"] for m in ms if m.get("role") == "leader"]
        terms = [m.get("term") or 0 for m in ms]
        cur = (tuple(lead), max(terms))
        if leaders.get(g["chain"]) != cur:
            if g["chain"] in leaders:
                changes.append((now, g["chain"], leaders[g["chain"]], cur))
            leaders[g["chain"]] = cur
        # A leader on another site; a deposed leader on the faulted site, which hears no higher term, may still
        # report itself leader of its old term beside it.
        if start and now >= start and g["chain"] not in moved and any(x != site for x in lead):
            moved[g["chain"]] = now - start
print("leader changes (time on member clocks, leaders before -> after, max term):")
for now, chain, a, b in changes:
    rel = f"{(now - start) / 1000:+.1f} s" if start else ""
    print(f"  {rel:>9} {chain}: {list(a[0])} t{a[1]} -> {list(b[0])} t{b[1]}")
print("first sample with a leader off the faulted site, after the fault started:")
for chain in sorted(leaders):
    print(f"  {chain}: {moved[chain] / 1000:.1f} s" if chain in moved else f"  {chain}: never")


def stats(path):
    if not path.exists():
        return {}
    out = {}
    for line in path.read_text().splitlines():
        for s in json.loads(line)["stats"]:
            out.setdefault((s["chain"], s["site"]), []).append(s)
    return out


before = stats(step / "stats-at-fault.jsonl")
after = stats(step / "stats.jsonl")
for label, d in [("before the fault", before), ("from the fault to the end", after)]:
    if not d:
        continue
    allc = sorted(c for v in d.values() for s in v for c in (s.get("commitMs") or []))
    print(f"commit latency {label}: n {len(allc)}, p50 {pct(allc, 50)}, p99 {pct(allc, 99)}, max {allc[-1] if allc else None} ms")
    for (chain, s), v in sorted(d.items()):
        c = sorted(x for e in v for x in (e.get("commitMs") or []))
        alarms = [a for e in v for a in (e.get("alarms") or [])]
        failed = sum(e.get("failedCalls") or 0 for e in v)
        if c or alarms or failed:
            print(f"  {chain} site {s}: n {len(c)} p50 {pct(c, 50)} p99 {pct(c, 99)} max {c[-1] if c else None}, "
                  f"failed calls {failed}, alarms {len(alarms)}{': ' + alarms[-1][:140] if alarms else ''}")
