#!/usr/bin/env python3
"""acctstop.py -> B0-30 short run's stop rule on the account meter (operator, 2026-11-16): every STEP_S (300 s) it
reads tests/fanout/acctcost.py twice, the run's window (from tests/am1/raw/$B030_RUN.start) and today's UTC window, and
appends one row to tests/am1/raw/$B030_RUN-acct.jsonl. It writes tests/am1/raw/b030-STOP when the run's cost passes
RUN_USD (150) or the account's UTC day passes DAY_USD (170). With PREFIX (b030r5: graft-m1, RUN_USD 60), the run's
cost is its install's scripts only (acctcost.py's prefix_usd), since other installs run slots alongside it. acctcost.py's last reading lags Cloudflare's analytics by
a few minutes, so the stop lands a few minutes after the line [inference]. Ends when tests/am1/raw/b030-run.done exists.
Runs from the worktree root; token from the environment, by header only (acctcost.py's own rule).
    B030_RUN=b030r4 python3 tests/am1/b030split/acctstop.py"""
import datetime as D, json, os, subprocess, sys, time

RAW = "tests/am1/raw"
RUN = os.environ.get("B030_RUN", "b030r4")
RUN_USD = float(os.environ.get("RUN_USD", 150))
DAY_USD = float(os.environ.get("DAY_USD", 170))
STEP_S = int(os.environ.get("STEP_S", 300))
STOP = f"{RAW}/b030-STOP"


PREFIX = os.environ.get("PREFIX", "")


def acct(a, b=None):
    out = subprocess.run([sys.executable, "tests/fanout/acctcost.py", a] + ([b] if b else []), capture_output=True,
                         text=True, timeout=240, env={**os.environ, "PREFIX": PREFIX}).stdout.splitlines()
    return json.loads(out[0]) if out else {}


def once():
    start = open(f"{RAW}/{RUN}.start").read().strip()
    now = D.datetime.now(D.UTC).replace(microsecond=0)
    run, day = acct(start), acct(now.strftime("%Y-%m-%dT00:00:00Z"))
    if PREFIX:
        run = {**run, "usd": run.get("prefix_usd"), "account": run.get("usd")}
    r = {"at": now.strftime("%Y-%m-%dT%H:%M:%SZ"), "run": run.get("usd"), "day": day.get("usd"), "runUsd": RUN_USD,
         "dayUsd": DAY_USD, "runParts": {k: v for k, v in run.items() if k not in ("from", "to", "usd")}}
    why = ([f"run ${run['usd']} past ${RUN_USD}"] if (run.get("usd") or 0) > RUN_USD else []) + \
          ([f"account day ${day['usd']} past ${DAY_USD}"] if (day.get("usd") or 0) > DAY_USD else [])
    r["stop"] = bool(why)
    open(f"{RAW}/{RUN}-acct.jsonl", "a").write(json.dumps(r) + "\n")
    print(json.dumps(r), flush=True)
    if why and not os.path.exists(STOP):
        open(STOP, "w").write(f"acctstop: {'; '.join(why)} at {r['at']}\n")


if __name__ == "__main__":
    while not os.path.exists(f"{RAW}/b030-run.done"):
        try:
            once()
        except Exception as e:  # a failed reading never ends the guard
            print(json.dumps({"error": str(e)[:200]}), flush=True)
        time.sleep(STEP_S)
