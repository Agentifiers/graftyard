#!/usr/bin/env python3
"""hourly.py [HOUR_ISO] -> one line for B0-30's hypothesis (ii) (operator, 2026-11-15): for the hour starting at
HOUR_ISO (default: the last full hour), from Workers Logs sampled 3 minutes at :20 on every site, the git cache shard's
/validate p50 (pushes; the derived promotion validates have no `tipsn`), the PrSync test merge /merge p50, the push rate from the load's minute rows,
and each site shard's /status `procs` top processes by CPU. Appends the line to tests/am1/raw/b030r2-hourly.jsonl.
"""
import datetime as D, json, os, re, subprocess, sys
RAW = "tests/am1/raw"
RUN = os.environ.get("B030_RUN", "b030r2")
REPO = json.load(open(f"{RAW}/b030-repo.json"))["repo"]
now = D.datetime.now(D.UTC)
h = D.datetime.fromisoformat(sys.argv[1]) if len(sys.argv) > 1 else now.replace(minute=0, second=0, microsecond=0) - D.timedelta(hours=1)
h = h.replace(tzinfo=D.UTC) if h.tzinfo is None else h
a = int(h.timestamp()) + 20 * 60
def logs(script, needle):
    out = subprocess.run(["python3", "tests/am1/b030/logs.py", script, str(a), str(a + 180), needle], capture_output=True,
                         text=True, env=dict(os.environ, STEP="30"), timeout=240).stdout
    return [json.loads(l) for l in out.splitlines() if l.strip()]
p50 = lambda w: sorted(w)[len(w) // 2] if w else None
w0 = D.datetime.fromtimestamp(a, D.UTC)
row = {"hour": h.strftime("%Y-%m-%dT%H:%MZ"), "sampled": f"{w0:%H:%M}-{w0 + D.timedelta(minutes=3):%H:%M}Z"}
for s in "abc":
    # The shard object's log line is its own URL; `disk=1` is on its call to the container and is not logged.
    v = [e["wall"] for e in logs(f"graft-m1-site-{s}", f"shard/validate?repo={REPO}&tipsn") if e.get("wall") is not None]
    m = [e["wall"] for e in logs(f"graft-m1-site-{s}", f"shard/merge?repo={REPO}") if e.get("wall") is not None]
    if v or m:
        row[s] = {"validate": [len(v), p50(v)], "merge": [len(m), p50(m)]}
mins = [json.loads(l) for l in open(f"{RAW}/b030-load.jsonl") if '"kind":"minute"' in l]
t = lambda x: D.datetime.fromisoformat(x.replace("Z", "+00:00"))
inh = [r for r in mins if h <= t(r["at"]) < h + D.timedelta(hours=1)]
if len(inh) > 1:
    row["pushesPerMin"] = round((inh[-1]["pushes"] - inh[0]["pushes"]) / ((t(inh[-1]["at"]) - t(inh[0]["at"])).total_seconds() / 60), 1)
# `/procs` is a snapshot of the git processes running now: six snapshots 10 s apart, each process's cpu_s at its
# last sight, summed by command, top 3 per site.
import time
seen = {}
for _ in range(6):
    st = subprocess.run(["node", "tests/am1/mb3/shardstatus.mjs", str(REPO)], capture_output=True, text=True, timeout=120).stdout
    for l in st.splitlines():
        r = json.loads(l)
        try:
            b = json.loads(r["body"])
        except ValueError:
            continue
        for pr in ((b.get("procs") or {}).get("procs") or []):
            seen[(r["site"], pr.get("pid"))] = (" ".join(str(pr.get("cmd", "")).split()[:3]), pr.get("cpu_s") or 0)
    time.sleep(10)
by = {}
for (site, _), (cmd, cpu) in seen.items():
    by.setdefault(site, {}).setdefault(cmd, 0.0)
    by[site][cmd] += cpu
row["procs"] = {s: sorted(((round(c, 1), k) for k, c in v.items()), reverse=True)[:3] for s, v in sorted(by.items())}
row["procsSnapshots"] = 6
print(json.dumps(row))
open(f"{RAW}/{RUN}-hourly.jsonl", "a").write(json.dumps(row) + "\n")
