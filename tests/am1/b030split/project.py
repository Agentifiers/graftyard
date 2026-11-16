#!/usr/bin/env python3
"""project.py -> the B0-30 stop rule from data (operator, 2026-11-15): from the last HOURS (3) hours of the load's
minute rows, its push rate, then the pushes owed at day 30 if that rate holds, the drain's wall time at the same rate,
and the total cost: the run-only meter's last reading, plus its hourly rate while the load ran over the active time and
the drain, plus what is left of the split. Prints one JSON line with `stop` true when the total passes STOP_USD (from
tests/am1/raw/b030-stop-usd) or the drain passes DRAIN_MAX_H (24).

The split (operator, 2026-11-16): the flat $58 it used to add is replaced by the split's measured spend and an estimate
of what is left. The meter already holds what the split has spent, so only the rest is added. The run rate skips meter
intervals that overlap the split, from its start (the first watch row with a child) to the slim pass's end, as it
skips pauses: at 00:26Z the child's restore lifted the rate to $8.17 an hour, and the old rule projected $316.2. The
split's spend so far is the meter's rise since the last row before it started, less the run rate over that time. Left
of it once the restore is done is the slim pass: SLIM_EST_USD, smoke 1's measured slim of a 3.56 GB tier on its three
parent members ($20.04, 2026-11-13 20:22Z to 22:57Z) scaled to this child's restored 3.24 GB, less what the slim has
spent from the first meter row after `MetaDropped`. Past that estimate, it adds an hour of the slim's own rate on each
reading and says so (`slimOverEstimate`)."""
import datetime as D, json, os, re

RAW = "tests/am1/raw"
HOURS = float(os.environ.get("HOURS", 3))
DRAIN_MAX_H = 24.0
SLIM_EST_USD = round(20.04 * 3_241_503_104 / 3.56e9, 2)
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
# Only intervals the load ran through: a monitor interval that overlaps a pause (a gap of 2 min or more in the minute
# rows, or the time since the last minute row while stopped) is skipped, since it bills almost nothing. Counting one
# from the 16:16Z pause left the rate at $1.88 an hour against a running $2.05 until it left the window at 19:32Z.
pauses = [(t(a["at"]), t(b["at"])) for a, b in zip(rows, rows[1:]) if (t(b["at"]) - t(a["at"])).total_seconds() >= 120]
if (D.datetime.now(D.UTC) - t(last["at"])).total_seconds() >= 120:
    pauses.append((t(last["at"]), D.datetime.max.replace(tzinfo=D.UTC)))
# The split's window, from the watcher's rows: start (a child), MetaDropped, and the slim pass's end.
start = dropped = done = None
for l in open(f"{RAW}/b030r2-watch.jsonl"):
    if '"kind":"split"' not in l:
        continue
    r = json.loads(l); sp = r["split"]
    start = start or (t(r["at"]) if sp.get("child") else None)
    dropped = dropped or (t(r["at"]) if sp.get("dropped") else None)
    done = done or (t(r["at"]) if (sp.get("slim") or {}).get("done") else None)
split_win = [(start, done or D.datetime.max.replace(tzinfo=D.UTC))] if start else []
ran = [(a, b) for a, b in zip(mon, mon[1:]) if not any(p0 < b[0] and a[0] < p1 for p0, p1 in pauses + split_win)]
ran = [x for x in ran if x[1][0] >= ran[-1][1][0] - D.timedelta(hours=HOURS)] if ran else []
h = sum((b[0] - a[0]).total_seconds() for a, b in ran) / 3600
usd_h = sum(b[1] - a[1] for a, b in ran) / h if h else 0.0
spent = mon[-1][1]


def excess(since):
    """The meter's rise from its last row at or before `since` (or first after, `after`) to now, less the run rate."""
    return lambda pick: (lambda m0: spent - m0[1] - usd_h * (mon[-1][0] - m0[0]).total_seconds() / 3600)(pick(since))


before = lambda at: [m for m in mon if m[0] <= at][-1]
after = lambda at: next((m for m in mon if m[0] >= at), mon[-1])
split_so_far = excess(start)(before) if start else 0.0
slim_so_far = excess(dropped)(after) if dropped else 0.0
over = bool(dropped and not done and slim_so_far > SLIM_EST_USD)
if done:
    split_left = 0.0
elif not dropped:
    split_left = SLIM_EST_USD  # the restore's rest is in the meter as it runs; the slim's estimate is all ahead
elif over:
    # Past the estimate: an hour of the slim's own rate each reading, until the pass ends.
    m0 = after(dropped); hh = (mon[-1][0] - m0[0]).total_seconds() / 3600
    split_left = slim_so_far / hh if hh else 0.0
else:
    split_left = SLIM_EST_USD - slim_so_far
total = spent + usd_h * (active_left_h + drain_h) + split_left
stop_usd = float(open(f"{RAW}/b030-stop-usd").read().strip())
print(json.dumps({
    "at": D.datetime.now(D.UTC).strftime("%Y-%m-%dT%H:%M:%SZ"), "day": round(day, 3), "ratePerMin": round(rate, 1),
    "windowMin": round(mins), "owedNow": round(owed_now), "owedAtDay30": round(owed30), "drainH": round(drain_h, 1),
    "spent": spent, "usdPerH": round(usd_h, 2), "splitSoFar": round(split_so_far, 2), "slimSoFar": round(slim_so_far, 2),
    "slimEst": SLIM_EST_USD, "splitLeft": round(split_left, 2), "slimOverEstimate": over, "total": round(total, 1), "stopUsd": stop_usd,
    "stop": total > stop_usd or drain_h > DRAIN_MAX_H}))
