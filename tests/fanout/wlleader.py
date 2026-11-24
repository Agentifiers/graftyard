#!/usr/bin/env python3
"""wlleader.py NAME FROM_ISO TO_ISO OUT.json.xz [SERVICE] -> every Workers Logs event of one Durable Object over a
window: each invocation's trigger, CPU and wall time, outcome and script version, and each console line, joined by
request id. NAME is the object's name, such as `repo:131406#1#f0`; its id comes from `wlobj.object_id`. Queries in
500 ms chunks and records how many came back full (2,000 events), so a gap is visible. SERVICE defaults to
graft-m2b-site-a. Token from the environment, as a header only; no account field is written.
"""
import json, lzma, sys
sys.path.insert(0, 'tests/fanout'); import wlobj
name, a, b, out = sys.argv[1:5]
svc = sys.argv[5] if len(sys.argv) > 5 else "graft-m2b-site-a"
do = wlobj.object_id(name, a, b)
rows, t, end, full = [], wlobj.ms(a), wlobj.ms(b), 0
step = 500
while t < end:
    body = {"queryId": "q", "timeframe": {"from": t, "to": min(end, t + step)}, "view": "events", "limit": 2000,
            "parameters": {"filters": wlobj.filters(svc, do), "limit": 2000}}
    ev = wlobj.post(wlobj.URL, body)["result"].get("events", {}).get("events", [])
    if len(ev) >= 2000: full += 1
    for e in ev:
        w, m = e.get("$workers") or {}, e.get("$metadata") or {}
        rows.append({"ts": e["timestamp"], "req": m.get("requestId"), "trigger": m.get("trigger"),
                     "msg": (m.get("message") or "")[:2000], "level": m.get("level"),
                     "wall": w.get("wallTimeMs"), "cpu": w.get("cpuTimeMs"), "outcome": w.get("outcome"),
                     "type": w.get("eventType"), "version": (w.get("scriptVersion") or {}).get("id")})
    t += step
with lzma.open(out, "wt") as f:
    json.dump({"name": name, "from": a, "to": b, "objectId": do, "chunksFull": full, "rows": rows}, f)
print(name, len(rows), "events, full chunks", full)
