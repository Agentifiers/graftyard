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
alarm invocations (each `setAlarm` is one row written). Since R23 (`RunnerBox`, `workers/runners/box.mjs`) it also
counts, per image, the starts that launched, exited within 3 s, or gave up: the canary check before a
`graftyard runners upgrade` goes to 100% (`decisions/DECISIONS.md`, "`r23-runnerbox`"). Prints one JSON summary. The first `BURST` Sandboxes (default
20, the cap) are reported apart from the later ones, which each start as a slot frees. Account and token from the
environment (`CLOUDFLARE_ACCOUNT_ID`, `CLOUDFLARE_API_TOKEN`), the token only as a header. The summary ends with
`chunks`, `failed` and `complete` (`runnerlogs.py`); the exit is 3 when any hour of the window never answered.
"""
import json, os, statistics, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import runnerlogs  # noqa: E402
from datetime import datetime, timezone

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
a, b = sys.argv[1], sys.argv[2]
burst_n = int(os.environ.get("BURST", "20"))
ms = lambda s: int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
iso = lambda t: datetime.fromtimestamp(t / 1000, timezone.utc).strftime("%H:%M:%S")
NO_INSTANCE = "no container instance that can be provided"
RATE = "too many containers per second"


# The events view answers at most 100 a query, newest first, and one walk back through a busy window can end early
# (the F80 burst: 990 of its events, 14 of 50 `launched` lines). `runnerlogs.fetch` reads the window in chunks of at
# most 1 h, each with its own deadline, each query with up to 3 tries and backoff, splits busy spans down to 30 s slices, and names any chunk
# that never answered, so one slow API call no longer fails the whole check (both canary wakes on 2026-11-20).
events, failed = runnerlogs.fetch(ms(a), ms(b), runnerlogs.api_query(acct, tok),
                                 progress=lambda l: print(l, file=sys.stderr, flush=True))

addressed, errors, destroyed, alarms, cancelled, launched, by_image = {}, {}, set(), 0, 0, {}, {}
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
    # `box.mjs` ends each start's line with ", image <ref>".
    for kind, prefix in (("launched", "graft-runners: launched"), ("exited", "graft-runners: runner exited within"),
                         ("gave_up", "graft-runners: gave up")):
        if msg.startswith(prefix) and ", image " in msg:
            img = (m.get("message") or "").rsplit(", image ", 1)[1].split(";")[0]
            by_image.setdefault(img, {"launched": 0, "exited": 0, "gave_up": 0})[kind] += 1
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
    "by_image": by_image,
    # The canary promotes only when this is true: a failed chunk's events are missing from every count above.
    **runnerlogs.chunks_line(ms(a), ms(b), failed),
}, indent=1))
sys.exit(0 if not failed else 3)
