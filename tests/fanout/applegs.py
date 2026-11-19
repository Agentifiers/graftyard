#!/usr/bin/env python3
"""applegs.py: where a leader's AppendEntries time goes, joining a tsave trace with Workers Logs (b033-m60, "Append legs").

  applegs.py fetch SVC FROM TO CALLER OUT.json    the callee site's /raft invocations from member CALLER (needs
                                                  CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_API_TOKEN; writes no account data)
  applegs.py join TSAVE CALLEE CALLER OUT.json    per call: the legs below, p10/p50/p90/p99

Each /raft call carries x-graft-call, x-graft-sent (the caller isolate's Date.now() at CallSent) and x-graft-clock (its
member clock, raised by peer stamps; `workers/site/src/clock.rs`). Workers Logs gives the callee invocation's end on
Cloudflare's clock (`timestamp`) and its `wallTimeMs`, so its start is timestamp - wall. Legs, per call:
  stale      member clock - isolate clock at send: how far the caller's own Date.now() read behind (a busy isolate's clock
             reads its last I/O, `tests/am1/results/lease-clock.md`), which CallSent's stamp hides.
  out        callee invocation start (Cloudflare) - caller member clock at send: wire plus whatever held the call before
             the callee's invocation began, bounded below by the member clock running ahead.
  qin        callee handler entry (its isolate clock, `Entered`) - invocation start: the callee's turns ahead of it.
  hand       callee handler entry to return (`CallLeft`), one clock.
Whether Cloudflare's invocation start includes the wait at the object's input gate is not documented [unverified]."""
import json, lzma, os, sys, time, urllib.request
from datetime import datetime


def pc(v, p):
    return sorted(v)[min(len(v) - 1, int(p * len(v)))] if v else None


def fetch(svc, a, b, caller, out):
    ms = lambda s: int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
    url = f"https://api.cloudflare.com/client/v4/accounts/{os.environ['CLOUDFLARE_ACCOUNT_ID']}/workers/observability/telemetry/query"
    rows, t, b = [], ms(a), ms(b)
    while t < b:
        f = [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": svc},
             {"key": "$workers.event.request.url", "operation": "eq", "type": "string", "value": "https://member/raft"},
             {"key": "$workers.event.request.headers.x-graft-from", "operation": "eq", "type": "string", "value": caller}]
        body = {"queryId": "q", "timeframe": {"from": t, "to": min(b, t + 30000)}, "view": "events", "limit": 1000,
                "parameters": {"filters": f}}
        ev = []
        for _ in range(4):
            try:
                r = urllib.request.Request(url, data=json.dumps(body).encode(), method="POST", headers={
                    "authorization": f"Bearer {os.environ['CLOUDFLARE_API_TOKEN']}", "content-type": "application/json"})
                ev = json.load(urllib.request.urlopen(r, timeout=120))["result"].get("events", {}).get("events", [])
                break
            except Exception:
                time.sleep(3)
        for e in ev:
            w = e.get("$workers", {})
            h = w.get("event", {}).get("request", {}).get("headers", {})
            rows.append({"ts": e["timestamp"], "call": h.get("x-graft-call"), "sent": int(h.get("x-graft-sent", 0)),
                         "clock": int(h.get("x-graft-clock", 0)), "wall": w.get("wallTimeMs"), "cpu": w.get("cpuTimeMs")})
        t += 30000
    json.dump(rows, open(out, "w"))
    print(svc, caller, len(rows), "invocations")


def join(trace, callee, caller, logs):
    rows = set()
    for line in (lzma.open if trace.endswith(".xz") else open)(trace, "rt"):
        d = json.loads(line)
        if "rows" in d and d.get("site") in (callee, caller) and d.get("learner") is None:
            rows.update((d["site"],) + tuple(x) for x in d["rows"] if x[1] in (21, 23, 24))
    left = {x[4]: (x[1], x[5]) for x in rows if x[0] == callee and x[2] == 24 and x[3] == caller}
    sent = {x[4]: (x[1], x[5]) for x in rows if x[0] == caller and x[2] == 23 and x[3] == callee}
    ans = {x[4]: x[1] for x in rows if x[0] == caller and x[2] == 21 and x[3] == callee}
    J = []
    for r in json.load(open(logs)):
        c = int(r["call"] or 0)
        if c in left and c in sent and c in ans and r["clock"]:
            (lt, ent), (s, aw), start = left[c], sent[c], r["ts"] - r["wall"]
            J.append({"rtt": ans[c] - s, "stale": r["clock"] - s, "out": start - r["clock"], "qin": ent - start,
                      "hand": lt - ent, "awaiting": aw})
    print(f"{caller}->{callee}: {len(J)} calls joined")
    for k in ("rtt", "stale", "out", "qin", "hand", "awaiting"):
        v = [j[k] for j in J]
        print(f"  {k:8} p10 {pc(v, .1)} p50 {pc(v, .5)} p90 {pc(v, .9)} p99 {pc(v, .99)}")


if __name__ == "__main__":
    if sys.argv[1] == "fetch":
        fetch(*sys.argv[2:7])
    else:
        join(sys.argv[2], int(sys.argv[3]), int(sys.argv[4]), sys.argv[5])
