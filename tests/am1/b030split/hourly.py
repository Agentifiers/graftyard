#!/usr/bin/env python3
"""hourly.py [HOUR_ISO] -> one line for B0-30's hypothesis (ii) (operator, 2026-11-15): for the hour starting at
HOUR_ISO (default: the last full hour), from Workers Logs sampled 3 minutes at :20 on every site, the git cache shard's
/validate p50 split by disk=1 and not, the PrSync test merge /merge p50, the push rate from the load's minute rows,
and each site shard's /status `procs` top processes by CPU. Appends the line to tests/am1/raw/b030r2-hourly.jsonl.
"""
import datetime as D, json, os, re, subprocess, sys
RAW = "tests/am1/raw"
now = D.datetime.now(D.UTC)
h = D.datetime.fromisoformat(sys.argv[1]) if len(sys.argv) > 1 else now.replace(minute=0, second=0, microsecond=0) - D.timedelta(hours=1)
h = h.replace(tzinfo=D.UTC) if h.tzinfo is None else h
a = int(h.timestamp()) + 20 * 60
def logs(script, needle):
    out = subprocess.run(["python3", "tests/am1/b030/logs.py", script, str(a), str(a + 180), needle], capture_output=True,
                         text=True, env=dict(os.environ, STEP="30"), timeout=240).stdout
    return [json.loads(l) for l in out.splitlines() if l.strip()]
p50 = lambda w: sorted(w)[len(w) // 2] if w else None
row = {"hour": h.strftime("%Y-%m-%dT%H:00Z"), "sampled": "3 min at :20"}
for s in "abc":
    v = [e for e in logs(f"graft-m1-site-{s}", "shard/validate?repo=1048590&tipsn") if e.get("wall") is not None]
    disk = [e["wall"] for e in v if "disk=1" in e.get("trigger", "") + e.get("msg", "")]
    other = [e["wall"] for e in v if "disk=1" not in e.get("trigger", "") + e.get("msg", "")]
    m = [e["wall"] for e in logs(f"graft-m1-site-{s}", "shard/merge?repo=1048590") if e.get("wall") is not None]
    if v or m:
        row[s] = {"validateDisk": [len(disk), p50(disk)], "validateOther": [len(other), p50(other)], "merge": [len(m), p50(m)]}
mins = [json.loads(l) for l in open(f"{RAW}/b030-load.jsonl") if '"kind":"minute"' in l]
t = lambda x: D.datetime.fromisoformat(x.replace("Z", "+00:00"))
inh = [r for r in mins if h <= t(r["at"]) < h + D.timedelta(hours=1)]
if len(inh) > 1:
    row["pushesPerMin"] = round((inh[-1]["pushes"] - inh[0]["pushes"]) / ((t(inh[-1]["at"]) - t(inh[0]["at"])).total_seconds() / 60), 1)
st = subprocess.run(["node", "tests/am1/mb3/shardstatus.mjs", "1048590"], capture_output=True, text=True, timeout=120).stdout
row["procs"] = {}
for l in st.splitlines():
    r = json.loads(l)
    try:
        b = json.loads(r["body"]) if r["body"].endswith("}") else {}
    except ValueError:
        b = {}
    procs = b.get("procs")
    if procs is None:
        mm = re.search(r'"procs":(\[.*?\])', r["body"])
        procs = mm and mm[1]
    row["procs"][r["site"]] = procs if procs is not None else r["body"][:200]
print(json.dumps(row))
open(f"{RAW}/b030r2-hourly.jsonl", "a").write(json.dumps(row) + "\n")
