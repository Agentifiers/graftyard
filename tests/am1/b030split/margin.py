#!/usr/bin/env python3
"""margin.py -> B0-30 rerun (operator, 2026-11-16): site c's parent member's place in the log, from the monitor's rows
(tests/am1/raw/b030-monitor.jsonl). Its margin is its applied index less the lowest truncatedTo of the other two members:
once the leader's archive truncates past the entries site c still needs, site c restores from the archive. Prints one
JSON line: lag behind the leader, its and the leader's apply rates over the last hour, the margin, the margin's hourly
trend (a least-squares slope over the last HOURS, default 2, of rows), and the projected crossing time at that trend
(null while the trend is flat or rising). `restoring` is set when site c's applied fell or its first stored log row
jumped past its last applied index, the signs of a restore.
  python3 tests/am1/b030split/margin.py"""
import datetime as D, json, os

HOURS = float(os.environ.get("HOURS", 2))
t = lambda s: D.datetime.fromisoformat(s.replace("Z", "+00:00"))
rows = []
for l in open("tests/am1/raw/b030-monitor.jsonl"):
    r = json.loads(l.replace("\x00", "") or "{}")
    m = {x["site"]: x for x in r.get("members", []) if "applied" in x}
    if len(m) < 3:
        continue
    lead = max(m.values(), key=lambda x: x["applied"])
    rows.append({"at": t(r["at"]), "c": m[2], "lead": lead,
                 "margin": m[2]["applied"] - min(m[0]["truncatedTo"], m[1]["truncatedTo"])})
last = rows[-1]
win = [r for r in rows if r["at"] >= last["at"] - D.timedelta(hours=HOURS)]
xs = [(r["at"] - win[0]["at"]).total_seconds() / 3600 for r in win]
ys = [r["margin"] for r in win]
n, mx, my = len(xs), sum(xs) / len(xs), sum(ys) / len(ys)
sxx = sum((x - mx) ** 2 for x in xs)
slope = sum((x - mx) * (y - my) for x, y in zip(xs, ys)) / sxx if sxx else 0.0
hour = [r for r in rows if r["at"] >= last["at"] - D.timedelta(hours=1)]
rate = lambda key: ((hour[-1][key]["applied"] - hour[0][key]["applied"]) /
                    ((hour[-1]["at"] - hour[0]["at"]).total_seconds() / 60)) if len(hour) > 1 else None
prev = rows[-2] if len(rows) > 1 else last
restoring = last["c"]["applied"] < prev["c"]["applied"] or (last["c"].get("logFirst") or 0) > prev["c"]["applied"] + 1
cross = (last["at"] + D.timedelta(hours=last["margin"] / -slope)) if slope < 0 and last["margin"] > 0 else None
print(json.dumps({
    "at": last["at"].strftime("%Y-%m-%dT%H:%M:%SZ"), "lag": last["lead"]["applied"] - last["c"]["applied"],
    "cPerMin": round(rate("c"), 1) if rate("c") is not None else None,
    "leaderPerMin": round(rate("lead"), 1) if rate("lead") is not None else None,
    "margin": last["margin"], "marginPerH": round(slope), "windowH": HOURS, "rows": n,
    "crossAt": cross.strftime("%Y-%m-%dT%H:%MZ") if cross else None, "crossed": last["margin"] <= 0,
    "restoring": restoring, "cSqliteFull": last["c"].get("sqliteFull"), "cDb": last["c"].get("dbBytes")}))
