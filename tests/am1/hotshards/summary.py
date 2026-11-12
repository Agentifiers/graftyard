#!/usr/bin/env python3
"""hot-shards: the numbers of one journey-9 phase on graft-m3c, from the drivers' raw logs.

    python3 tests/am1/hotshards/summary.py FROM_ISO TO_ISO

Reads tests/am1/raw/hotshards/pushes.jsonl (push latency and results), monitor.jsonl (each member's log above the
last `Promoted`, `promotedTo`, `archivedTo`, the leader, and the shards' catch-up reports with `held_max`), and
faults.log (leader transfers and the shard kill), and prints one Markdown block. PUSHES and FAULTS name other files there, such as j9b-pushes.jsonl and faultsb.log.
"""
import json, os, re, sys

R = "tests/am1/raw/hotshards"
frm, to = sys.argv[1], sys.argv[2]
inside = lambda at: frm <= at <= to


def q(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(len(xs) * p))] if xs else None


pushes = [json.loads(l) for l in open(f"{R}/{os.environ.get('PUSHES', 'pushes.jsonl')}")]
pushes = [p for p in pushes if inside(p["at"]) and p.get("mode") == "j9"]
res = {}
for p in pushes:
    res[p["res"]] = res.get(p["res"], 0) + 1
ok = [p["ms"] for p in pushes if p["res"] == "ok"]
span = 0
if pushes:
    from datetime import datetime
    t = lambda s: datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp()
    span = t(pushes[-1]["at"]) - t(pushes[0]["at"])
print(f"pushes: {len(pushes)} in {span:.0f} s ({len(pushes) / max(span, 1):.2f}/s), results {res}")
print(f"ok latency ms: p50 {q(ok, .5)} p90 {q(ok, .9)} p99 {q(ok, .99)} max {max(ok) if ok else None}")
# Per 10 minutes, so a fault's effect shows.
buckets = {}
for p in pushes:
    k = p["at"][:15] + "0"
    buckets.setdefault(k, []).append(p)
for k, ps in sorted(buckets.items()):
    oks = [p["ms"] for p in ps if p["res"] == "ok"]
    print(f"  {k[11:]}Z  {len(ps):4d} pushes, {len(oks):4d} ok, p50 {q(oks, .5)} p99 {q(oks, .99)} max {max(oks) if oks else None}")

rows = [json.loads(l) for l in open(f"{R}/monitor.jsonl")]
rows = [r for r in rows if inside(r["at"])]
print("monitor (least member above the last Promoted; leader's promotedTo, archivedTo, applied):")
held = []
leaders = []
for r in rows:
    ms = [m for m in r["members"] if "unpromoted" in m]
    lead = next((m for m in ms if m.get("role") == "Leader"), None)
    if lead and (not leaders or leaders[-1][1] != lead["site"]):
        leaders.append((r["at"], lead["site"]))
    if ms and lead:
        print(f"  {r['at'][11:19]}Z leader {lead['site']} applied {lead['applied']} above-promoted "
              f"{min(m['unpromoted'] for m in ms) / 1e9:.3f} GB log {lead['logBytes'] / 1e9:.3f} GB "
              f"promotedTo {lead['promotedTo']} archivedTo {lead['archivedTo']}")
    for c in r.get("catchups") or []:
        v = c.get("v")
        if isinstance(v, dict) and v.get("held_max") is not None:
            held.append(v["held_max"])
print(f"leaders seen: {leaders}")
print(f"catch-up held_max over {len(held)} reports: max {max(held) if held else None} bytes")
print("faults:")
for l in open(f"{R}/{os.environ.get('FAULTS', 'faults.log')}"):
    m = re.match(r"(\S+) (.*)", l.strip())
    if m and inside(m.group(1)):
        print(f"  {l.strip()}")
