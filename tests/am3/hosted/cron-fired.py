#!/usr/bin/env python3
"""OPEN-QUESTIONS R52: waits for a script's first scheduled run in Workers Logs, so no hosted row starts on a cron that
has not fired. A new trigger fired 4.5 to 10.3 minutes after it was set, reset or not (`tests/am3/raw/r52/`), inside
Cloudflare's documented 15 minutes, so the wait is the documented bound; only if none comes in it is the trigger reset
(a yearly cron, then the wanted one) and waited for once more.

    cron-fired.py SCRIPT CRON [--since MS] [--wait-s 900] [--tries 1]

Uses CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_API_TOKEN (header only). Prints one JSON line per reset and the first run;
exit 0 once a scheduled run at or after --since (default: now) is logged, exit 1 after --tries resets with none.
"""
import argparse, json, sys, time, urllib.request, os

p = argparse.ArgumentParser()
p.add_argument("script"); p.add_argument("cron")
p.add_argument("--since", type=int); p.add_argument("--wait-s", type=int, default=900); p.add_argument("--tries", type=int, default=1)
a = p.parse_args()
acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
B = f"https://api.cloudflare.com/client/v4/accounts/{acct}"


def call(method, path, body=None):
    r = urllib.request.Request(B + path, data=None if body is None else json.dumps(body).encode(), method=method,
                               headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
    return json.load(urllib.request.urlopen(r, timeout=60))


def runs(since):
    body = {"queryId": "q", "timeframe": {"from": since, "to": int(time.time() * 1000)}, "view": "events", "limit": 10,
            "parameters": {"filters": [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": a.script},
                                       {"key": "$workers.eventType", "operation": "eq", "type": "string", "value": "scheduled"}]}}
    ev = call("POST", "/workers/observability/telemetry/query", body).get("result", {}).get("events", {}).get("events", [])
    return sorted(e["timestamp"] for e in ev)


def out(**k):
    print(json.dumps({"at": time.strftime("%H:%M:%SZ", time.gmtime()), **k}), flush=True)


since = a.since or int(time.time() * 1000)
for t in range(a.tries + 1):
    end = time.time() + a.wait_s
    while time.time() < end:
        r = runs(since)
        if r:
            out(fired=time.strftime("%H:%M:%SZ", time.gmtime(r[0] / 1000)), resets=t)
            sys.exit(0)
        time.sleep(20)
    if t == a.tries:
        break
    call("PUT", f"/workers/scripts/{a.script}/schedules", [{"cron": "0 0 1 1 *"}])
    time.sleep(3)
    s = call("PUT", f"/workers/scripts/{a.script}/schedules", [{"cron": a.cron}])
    out(no_run_in_s=a.wait_s, reset=t + 1, schedules=[x["cron"] for x in s["result"]["schedules"]])
out(error=f"no scheduled run after {a.tries} resets")
sys.exit(1)
