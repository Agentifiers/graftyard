#!/usr/bin/env python3
"""judge.py CELL.jsonl [...] -> `cold-write`'s hosted cell lines (tests/am1/results/b034-cw1.md).

Joins each POST the cell sent (`cell.mjs`, by `x-graft-client-push`) to the edge's push log line for it (`push_log`
in workers/edge/src/lib.rs: `{"push", "isolate", "open", "status", "timing", "client"}`) in Workers Logs, with the
colo from the reply's `cf-ray`. A cold first POST is the first push its isolate logged in the cell's window. Prints, per arm:
- cold first POSTs: front + gate median, p90, p99 and max, and where each gate came from (`gate_colo`, the data
  center's entry; `gate_iso`, the isolate's own; else a fresh read);
- warm POSTs (the git arm's second POST on its connection, and any POST on an isolate that had pushed before);
- every refusal by arm and cause, and the two safety counts: a POST in the fetch, other or ro arm whose gate came from
  the data center's entry, and a ro POST that was not refused 403.
Account and token from the environment, the token only as a header. SCRIPT (default graft-m1-edge) names the edge.
"""
import collections
import json
import os
import sys
import time
import urllib.request
from datetime import datetime

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
svc = os.environ.get("SCRIPT", "graft-m1-edge")
ms = lambda s: int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)


def q(a, b):
    body = {"queryId": "q", "timeframe": {"from": a, "to": b}, "view": "events", "limit": 500,
            "parameters": {"filters": [
                {"key": "$metadata.service", "operation": "eq", "type": "string", "value": svc},
                {"key": "push", "operation": "exists", "type": "string"}]}}
    for i in range(4):
        try:
            r = urllib.request.Request(
                f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
                data=json.dumps(body).encode(), method="POST",
                headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
            return json.load(urllib.request.urlopen(r, timeout=90))["result"]["events"]["events"]
        except Exception:
            if i == 3:
                raise
            time.sleep(3 * (i + 1))


def pushes(a, b):
    """The edge's push lines in a..b: client id -> (isolate, open, timestamp)."""
    out, lo = {}, a
    while lo < b:
        hi = min(b, lo + 60_000)
        while True:
            got = q(lo, hi)
            if len(got) < 500 or hi - lo <= 2_000:
                break
            hi = lo + (hi - lo) // 2
        for e in got:
            src = e.get("source") or {}
            client = src.get("client")
            if client:
                out[client] = (src.get("isolate"), src.get("open"), e.get("timestamp"))
        lo = hi
    return out


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(len(xs) * p))] if xs else None


def line(name, xs):
    if not xs:
        return f"{name}: none"
    return (f"{name}: n {len(xs)}, median {pct(xs, .5):.0f}, p90 {pct(xs, .9):.0f}, p99 {pct(xs, .99):.0f}, "
            f"max {max(xs):.0f} ms")


def served(st):
    return "colo" if "gate_colo" in st else "isolate" if "gate_iso" in st else "read"


def main():
    rows = [json.loads(l) for f in sys.argv[1:] for l in open(f)]
    if not rows:
        sys.exit("no rows")
    # From the edge day (SINCE), so an isolate's first push since the version went live is its first push at all.
    a = ms(os.environ["SINCE"]) if "SINCE" in os.environ else ms(min(r["at"] for r in rows)) - 180_000
    b = ms(max(r["at"] for r in rows)) + 60_000
    logged = pushes(a, b)
    posts = sorted((r for r in rows if r["step"].startswith("post")), key=lambda r: r["at"])
    seen_iso, cold, warm = set(), [], []
    missing = 0
    mine = {r["client"] for r in posts}
    firsts = {}
    for client, (iso, _, at) in sorted(logged.items(), key=lambda kv: kv[1][2]):
        firsts.setdefault(iso, client)
    print(f"isolates whose first push since SINCE was not the cell's: {sum(c not in mine for c in firsts.values())}")
    for r in posts:
        iso = logged.get(r["client"])
        if iso is None:
            missing += 1
            continue
        r["isolate"] = iso[0]
        first = firsts.get(iso[0]) == r["client"] if "SINCE" in os.environ else iso[0] not in seen_iso
        seen_iso.add(iso[0])
        (cold if first and r["step"] == "post" else warm).append(r)
    print(f"{len(posts)} POSTs, {len(logged)} edge push lines in the window, {missing} POSTs with no line")
    print(f"isolates {len(seen_iso)}, colos {dict(collections.Counter(r.get('colo') for r in posts))}")
    fg = lambda r: r["st"].get("front", 0) + r["st"].get("gate", 0)
    for arm in ("git", "fetch", "other", "ro"):
        c = [r for r in cold if r["arm"] == arm]
        print(line(f"cold first POST, {arm}, front+gate", [fg(r) for r in c]))
        print(f"  gates: {dict(collections.Counter(served(r['st']) for r in c))}")
        print(f"  {line('front', [r['st'].get('front', 0) for r in c])}; {line('gate', [r['st'].get('gate', 0) for r in c])}")
        print(f"  {line('client ms', [r['ms'] for r in c])}")
    g = [r for r in cold if r["arm"] == "git"]
    print(line("cold first POST after an advertisement (git), edge total", [r["st"].get("total", 0) for r in g]))
    w = [r for r in warm if r["arm"] == "git" and r["step"] == "post-warm"]
    print(line("warm POST (git, second on its connection), front+gate", [fg(r) for r in w]))
    print(f"  gates: {dict(collections.Counter(served(r['st']) for r in w))}")
    every = [r for r in posts if r["arm"] == "git" and r["step"] == "post"]
    print(line("every git first POST, front+gate", [fg(r) for r in every]))
    print(f"  gates: {dict(collections.Counter(served(r['st']) for r in every))}")
    print("refusals by arm and cause:")
    by = collections.Counter((r["arm"], r["step"], r["result"] or f"http {r['status']}") for r in rows
                             if r["step"].startswith("post") and r["result"] != "ok")
    for (arm, step, why), n in sorted(by.items()):
        print(f"  {n:4d} {arm} {step}: {why}")
    other = [r for r in rows if r["step"] in ("advert", "fetch") and r["status"] != 200]
    for r in other:
        print(f"  advert/fetch {r['arm']} {r['who']}: {r['status']}")
    bypass = [r for r in posts if r["arm"] in ("fetch", "other", "ro") and served(r["st"]) == "colo"]
    ro_ok = [r for r in posts if r["arm"] == "ro" and r["status"] != 403]
    print(f"fetch/other/ro POSTs whose gate came from the data center's entry: {len(bypass)}")
    print(f"ro POSTs not refused 403: {len(ro_ok)} of {sum(r['arm'] == 'ro' for r in posts)}")
    for r in bypass + ro_ok:
        print(f"  {r['arm']} {r['client']} {r['status']} {r['result']} {served(r['st'])}")


if __name__ == "__main__":
    main()
