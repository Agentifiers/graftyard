#!/usr/bin/env python3
"""sandboxstarts.py FROM_ISO TO_ISO -> how `graft-runners`' Sandbox starts waited for the platform in the window.

Reads the runner account's Workers Logs for `graft-runners` (events view, every event in the window) and, per Sandbox
(one Durable Object per runner, `SandboxStd3` or `SandboxStd4`):

- when the pass first addressed it (its `setSandboxName` RPC, or since F80 its `arm`), so the Sandboxes are ordered by
  start;
- its Sandbox SDK placement errors, "There is no container instance that can be provided" and "You are requesting too
  many containers per second", and how long after the address the last one came (the placement wait);
- whether `graft-runners` destroyed it, and, since F80, how many processes its alarm launched (`launch.mjs` logs one
  `graft-runners: launched` line per process it starts).

It also counts the delivery passes whose `waitUntil` the runtime cancelled at its 30 s limit, and the Sandbox objects'
alarm invocations (each `setAlarm` is one row written). Prints one JSON summary. The first `BURST` Sandboxes (default
20, the cap) are reported apart from the later ones, which each start as a slot frees. Account and token from the
environment (`CLOUDFLARE_ACCOUNT_ID`, `CLOUDFLARE_API_TOKEN`), the token only as a header.
"""
import json, os, statistics, sys, urllib.request
from datetime import datetime, timezone

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
a, b = sys.argv[1], sys.argv[2]
burst_n = int(os.environ.get("BURST", "20"))
ms = lambda s: int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
iso = lambda t: datetime.fromtimestamp(t / 1000, timezone.utc).strftime("%H:%M:%S")
NO_INSTANCE = "no container instance that can be provided"
RATE = "too many containers per second"


def query(lo, hi):
    body = {"queryId": "q", "timeframe": {"from": lo, "to": hi}, "view": "events", "limit": 100, "parameters": {
        "filters": [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": "graft-runners"}]}}
    r = urllib.request.Request(
        f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
        data=json.dumps(body).encode(), method="POST",
        headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
    return json.load(urllib.request.urlopen(r, timeout=60))["result"]["events"]["events"]


# The events view answers at most 100 a query, newest first: walk back through the window in slices.
events, seen, hi, lo = [], set(), ms(b), ms(a)
while hi > lo:
    got = query(lo, hi)
    new = [e for e in got if e["$metadata"]["id"] not in seen]
    seen.update(e["$metadata"]["id"] for e in new)
    events += new
    if len(got) < 100 or not new:
        break
    hi = min(e["timestamp"] for e in got) + 1

addressed, errors, destroyed, alarms, cancelled, launched = {}, {}, set(), 0, 0, {}
for e in sorted(events, key=lambda e: e["timestamp"]):
    m, w = e["$metadata"], e.get("$workers") or {}
    msg, obj = (m.get("message") or "").lower(), w.get("durableObjectId")
    if m.get("type") == "cf-worker-event":
        rpc = (w.get("event") or {}).get("rpcMethods") or []
        if obj and ("setSandboxName" in rpc or "arm" in rpc):
            addressed.setdefault(obj, e["timestamp"])
        if obj and "destroy" in rpc:
            destroyed.add(obj)
        alarms += obj is not None and w.get("eventType") == "alarm"
        continue
    if obj and msg.startswith("graft-runners: launched"):
        launched[obj] = launched.get(obj, 0) + 1
    if "waituntil() tasks did not complete" in msg:
        cancelled += 1
    for kind in (NO_INSTANCE, RATE):
        if obj and kind in msg:
            errors.setdefault(obj, {NO_INSTANCE: [], RATE: []})[kind].append(e["timestamp"])


def group(objs):
    waits = sorted(round((max(errors[o][NO_INSTANCE] + errors[o][RATE]) - addressed[o]) / 1000)
                   if o in errors else 0 for o in objs)
    return {
        "sandboxes": len(objs),
        "spread_s": round((addressed[objs[-1]] - addressed[objs[0]]) / 1000, 1) if objs else 0,
        "no_instance": sum(len(errors.get(o, {}).get(NO_INSTANCE, [])) for o in objs),
        "too_many_per_second": sum(len(errors.get(o, {}).get(RATE, [])) for o in objs),
        "with_rate_error": sum(1 for o in objs if errors.get(o, {}).get(RATE)),
        "placement_wait_s": {"p50": statistics.median(waits) if waits else 0, "max": waits[-1] if waits else 0,
                             "over_30": sum(1 for x in waits if x > 30), "all": waits},
    }


order = sorted(addressed, key=addressed.get)
print(json.dumps({
    "window": [a, b],
    "first_start": iso(addressed[order[0]]) if order else None,
    "burst": group(order[:burst_n]),
    "later": group(order[burst_n:]),
    "destroyed": len(destroyed),
    "delivery_passes_cancelled_at_30s": cancelled,
    "launched": len(launched),
    "launched_twice": sum(1 for n in launched.values() if n > 1),
    "sandbox_alarm_invocations": alarms,
}, indent=1))
