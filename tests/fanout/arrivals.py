#!/usr/bin/env python3
"""arrivals.py PREFIX FROM TO [OUT] -> one install's invocations per second by script, from Workers Logs, and the
harness-outage windows of a gate row (tests/fanout/README.md, "Harness outages on gate rows").

A second is an outage second when the main edge (PREFIX-edge) got 0 invocations while the site scripts (PREFIX-site-*)
got some: the orb could not reach the edge, and the sites kept serving. The edge's invocations are counted by the
second they started (event timestamp less `$workers.wallTimeMs`, from the events view), because Workers Logs stamps an
invocation at its end: on mt26 181 edge invocations started in the load's first second and none ended in it (wall p50
3,027 ms on cold isolates), which counting by end read as a dropped host (decisions/OPEN-QUESTIONS.md, G43). The sites
and the tools edge are counted from the calculations view, by end. Adjacent outage seconds join into one window.
Prints one JSON line per second to OUT (default stdout: {"t": iso, "edge": n, "sites": n, "tools": n}), then a summary:
the windows, their seconds, and their share of FROM..TO. The row is invalid and reruns when the share is over
MAX_SHARE. Account and token from the environment, the token only as a header.
"""
import json
import os
import sys
import urllib.request
from datetime import datetime, timezone

MAX_SHARE = 0.05  # over this share of the load window in outages, the row is invalid and reruns
# Under about one edge request a second, an edge-0 second is the row's own idle gap, not a dropped host: m3fg11 (one
# merge every 6 s) marked 54% of its seconds (tests/fanout/README.md, "Harness outages on gate rows", rule 5).
MIN_EDGE_RATE = 1.0
STEP = 1000
EVENTS_MAX = 2000  # the events view's largest page; a slice that fills it is split
SLICE = 5000  # ms of events a call asks for first, under EVENTS_MAX at full load (about 300 edge invocations a second)
TAIL = 60_000  # ms past TO to read ends from, so an invocation that started before TO and ran long is counted


def ms(s):
    return int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)


def iso(t):
    return datetime.fromtimestamp(t / 1000, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def post(body):
    acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
    r = urllib.request.Request(f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
                               data=json.dumps(body).encode(), method="POST",
                               headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
    return json.load(urllib.request.urlopen(r, timeout=90))["result"]


def by_start(events, a, b):
    """{second ms: count} of invocations that started in [a, b), from (end timestamp ms, wallTimeMs) pairs."""
    out = {}
    for ts, wall in events:
        t = ts - wall
        if a <= t < b:
            out[t - t % STEP] = out.get(t - t % STEP, 0) + 1
    return out


def edge_events(prefix, a, b):
    """[(end ms, wallTimeMs)] of every PREFIX-edge invocation (wall > 0 leaves out log-only events) that ended in [a, b)."""
    out, t, step = [], a, SLICE
    while t < b:
        u = min(b, t + step)
        body = {"queryId": "q", "timeframe": {"from": t, "to": u}, "view": "events", "limit": EVENTS_MAX,
                "parameters": {"filters": [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": f"{prefix}-edge"},
                                           {"key": "$workers.wallTimeMs", "operation": "gt", "type": "number", "value": 0}]}}
        ev = post(body)["events"]["events"]
        if len(ev) >= EVENTS_MAX and u - t > 50:
            step = max(50, (u - t) // 2)
            continue
        out += [(e["timestamp"], e["$workers"]["wallTimeMs"]) for e in ev]
        t, step = u, SLICE
    return out


def query(prefix, a, b):
    """{second ms: {service: count}} over [a, b) by end, at most 10 minutes a call (the view's 600 points at 1 s)."""
    out = {}
    t = a
    while t < b:
        u = min(b, t + 600 * STEP)
        body = {"queryId": "q", "timeframe": {"from": t, "to": u}, "view": "calculations", "limit": 2000, "granularity": STEP,
                "parameters": {"filters": [{"key": "$metadata.service", "operation": "starts_with", "type": "string", "value": f"{prefix}-"}],
                               "calculations": [{"operator": "count"}],
                               "groupBys": [{"type": "string", "value": "$metadata.service"}]}}
        res = post(body)["calculations"][0]
        for s in res.get("series", []):
            sec = ms(s["time"].replace(" ", "T") + "Z")
            for d in s["data"]:
                out.setdefault(sec, {})[d["groupKey"]] = out.get(sec, {}).get(d["groupKey"], 0) + int(d["count"])
        t = u
    return out


def series(prefix, counts, edge, a, b):
    """Rows per second: `edge` is {second ms: count} by start (`by_start`), `counts` the other scripts by end."""
    rows = []
    for t in range(a - a % STEP, b, STEP):
        c = counts.get(t, {})
        rows.append({"t": t, "edge": edge.get(t, 0),
                     "sites": sum(n for k, n in c.items() if k.startswith(f"{prefix}-site-")),
                     "tools": c.get(f"{prefix}-tools-edge", 0)})
    return rows


def outages(rows):
    """[(start ms, end ms)] of runs of seconds with edge 0 and sites > 0; end is exclusive."""
    wins, start = [], None
    for r in rows + [{"t": None, "edge": 1, "sites": 0}]:
        bad = r["edge"] == 0 and r["sites"] > 0
        if bad and start is None:
            start = r["t"]
        elif not bad and start is not None:
            wins.append((start, prev + STEP))
            start = None
        prev = r["t"]
    return wins


def summary(wins, a, b, rows=None):
    """The windows and their share; with `rows`, a row whose edge got under MIN_EDGE_RATE requests a second over the
    window is not judged by the rule: `outages` reads "n/a (low rate)" and nothing is removed."""
    if rows is not None:
        rate = sum(r["edge"] for r in rows) / max(1, (b - a) / 1000)
        if rate < MIN_EDGE_RATE:
            return {"outages": "n/a (low rate)", "edgePerS": round(rate, 2), "valid": True}
    secs = sum(e - s for s, e in wins) / 1000
    share = secs / ((b - a) / 1000)
    return {"outages": [[iso(s), iso(e)] for s, e in wins], "outageSeconds": secs, "share": round(share, 4),
            "valid": share <= MAX_SHARE}


def main(argv):
    prefix, a, b = argv[0], ms(argv[1]), ms(argv[2])
    edge = by_start(edge_events(prefix, a - a % STEP, b + TAIL), a - a % STEP, b)
    rows = series(prefix, query(prefix, a, b), edge, a, b)
    with (open(argv[3], "w") if len(argv) > 3 else sys.stdout) as f:
        for r in rows:
            f.write(json.dumps({**r, "t": iso(r["t"])}) + "\n")
    s = summary(outages(rows), a, b, rows)
    print(json.dumps(s), file=sys.stderr if len(argv) <= 3 else sys.stdout)
    return s


if __name__ == "__main__":
    if len(sys.argv) < 4:
        sys.exit(__doc__)
    main(sys.argv[1:])
