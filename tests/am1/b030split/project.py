#!/usr/bin/env python3
"""project.py -> the B0-30 stop rule from data (operator, 2026-11-15): from the last HOURS (3) hours of the load's
minute rows, its push rate, then the pushes owed at day 30 if that rate holds, the drain's wall time at the same rate,
and the total cost: the run-only meter's last reading plus its hourly rate over the active time and the drain, plus
SPLIT_USD (58) for the split. Prints one JSON line with `stop` true when the total passes STOP_USD (from
tests/am1/raw/b030-stop-usd) or the drain passes DRAIN_MAX_H (24)."""
import datetime as D, json, os, re

RAW = "tests/am1/raw"
HOURS = float(os.environ.get("HOURS", 3))
SPLIT_USD, DRAIN_MAX_H = 58.0, 24.0
SPEED, PUSH_PER_S, DAYS = 18, 1.6, 30
t = lambda s: D.datetime.fromisoformat(s.replace("Z", "+00:00"))

rows = [json.loads(l) for l in open(f"{RAW}/b030-load.jsonl") if '"kind": "minute"' in l or '"kind":"minute"' in l]
last = rows[-1]
since = t(last["at"]) - D.timedelta(hours=HOURS)
# The window's rate counts only minutes the load ran: a pause (a STOP) leaves a gap in the rows, skipped here.
win = [r for r in rows if t(r["at"]) >= since]
pushes = mins = 0
for a, b in zip(win, win[1:]):
    gap = (t(b["at"]) - t(a["at"])).total_seconds() / 60
    if gap < 2:
        pushes += b["pushes"] - a["pushes"]; mins += gap
rate = pushes / mins if mins else 0.0  # pushes a minute

st = json.load(open(f"{RAW}/b030-state.json"))
day = st["active"] / (86_400 / SPEED)
active_left_h = max(0.0, (DAYS - day) * 86_400 / SPEED / 3600)
owed_now = st.get("pushDue", 0)
owed30 = max(0.0, owed_now + (PUSH_PER_S * 60 - rate) * 60 * active_left_h)
drain_h = owed30 / (rate * 60) if rate else float("inf")

# The run-only meter: the monitor's last cost, and its hourly rate over the same window of rows.
mon = []
for l in open(f"{RAW}/b030r2-monitor.log"):
    m = re.match(r"(\S+) monitor day \S+ .* cost \$([\d.]+)", l)
    if m:
        mon.append((t(m[1]), float(m[2])))
w = [m for m in mon if m[0] >= mon[-1][0] - D.timedelta(hours=HOURS)]
usd_h = (w[-1][1] - w[0][1]) / ((w[-1][0] - w[0][0]).total_seconds() / 3600) if len(w) > 1 and w[-1][0] > w[0][0] else 0.0
spent = mon[-1][1]
total = spent + usd_h * (active_left_h + drain_h) + SPLIT_USD
stop_usd = float(open(f"{RAW}/b030-stop-usd").read().strip())
print(json.dumps({
    "at": D.datetime.now(D.UTC).strftime("%Y-%m-%dT%H:%M:%SZ"), "day": round(day, 3), "ratePerMin": round(rate, 1),
    "windowMin": round(mins), "owedNow": round(owed_now), "owedAtDay30": round(owed30), "drainH": round(drain_h, 1),
    "spent": spent, "usdPerH": round(usd_h, 2), "total": round(total, 1), "stopUsd": stop_usd,
    "stop": total > stop_usd or drain_h > DRAIN_MAX_H}))
