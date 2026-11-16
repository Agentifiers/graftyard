#!/usr/bin/env python3
"""runs.py FROM_ISO TO_ISO [FROM_ISO TO_ISO ...] -> B0-30 rerun (operator, 2026-11-16): each archive run of the group's
leader with the work it did and the holds inside it, then the correlation of holds with that work over each window.

- An archive run is consecutive `archive Archived` or `archive Promoted` lines from one site less than RUN_GAP_S (30)
  apart (holds.py counts `Archived` alone; a run here includes the promotions that lead it).
- Its work: `Archived` entries (sum of to - from + 1) and objects (keys), `Promoted` entries, and its length.
- Bytes archived are entries times the leader's mean row bytes in the window's nearest monitor row (logBytes /
  logRows), so they are an estimate, not a sum of objects [inference].
- Holds: StepDowns on the group's own members from 15 s before a run to 15 s after it, as holds.py counts them.
- Per window: Pearson and Spearman of holds against archived entries, against run length, and holds a minute of run.
Token from the environment, by header only."""
import datetime as D, json, os, re, statistics as S, sys, time, urllib.error, urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
URL = f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query"
ms = lambda iso: int(D.datetime.fromisoformat(iso.replace("Z", "+00:00")).timestamp() * 1000)
hm = lambda t: D.datetime.fromtimestamp(t / 1000, D.UTC).strftime("%H:%M:%S")
RUN_GAP_S = 30
ids = {l.strip() for l in open("tests/am1/raw/b030-objects.txt") if l.strip()}


def events(svc, needle, fr, to):
    out = []
    for t in range(fr, to, 120_000):
        body = {"queryId": "q", "timeframe": {"from": t, "to": min(to, t + 120_000)}, "view": "events", "limit": 100,
                "parameters": {"filters": [
                    {"key": "$metadata.service", "operation": "eq", "type": "string", "value": svc},
                    {"key": "$metadata.message", "operation": "includes", "type": "string", "value": needle}]}}
        req = urllib.request.Request(URL, data=json.dumps(body).encode(), method="POST",
                                     headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
        for i in range(3):
            try:
                out += json.load(urllib.request.urlopen(req, timeout=60))["result"]["events"]["events"]
                break
            except (urllib.error.URLError, TimeoutError):
                if i == 2:
                    raise
                time.sleep(5 * (i + 1))
    return [e for e in out if e.get("$workers", {}).get("durableObjectId") in ids]


def row_bytes(t):
    """The leader's mean log row bytes at the monitor row nearest `t`."""
    best = None
    for l in open("tests/am1/raw/b030-monitor.jsonl"):
        r = json.loads(l)
        d = abs(ms(r["at"]) - t)
        if best is None or d < best[0]:
            lead = [m for m in r.get("members", []) if m.get("role") == "Leader"] or r.get("members", [])
            if lead and lead[0].get("logRows"):
                best = (d, lead[0]["logBytes"] / lead[0]["logRows"])
    return best[1] if best else None


def rank(xs):
    o = sorted(range(len(xs)), key=lambda i: xs[i])
    r = [0.0] * len(xs)
    i = 0
    while i < len(o):
        j = i
        while j + 1 < len(o) and xs[o[j + 1]] == xs[o[i]]:
            j += 1
        for k in range(i, j + 1):
            r[o[k]] = (i + j) / 2
        i = j + 1
    return r


def pearson(x, y):
    if len(x) < 3 or S.pstdev(x) == 0 or S.pstdev(y) == 0:
        return None
    mx, my = S.mean(x), S.mean(y)
    return round(sum((a - mx) * (b - my) for a, b in zip(x, y)) / (len(x) * S.pstdev(x) * S.pstdev(y)), 2)


args = sys.argv[1:]
for w in range(0, len(args), 2):
    a, b = ms(args[w]), ms(args[w + 1])
    marks, downs = [], []
    for s in "abc":
        svc = f"graft-m1-site-{s}"
        for e in events(svc, "archive Archived", a, b) + events(svc, "archive Promoted", a, b):
            m = e["$metadata"]["message"]
            f, t = re.search(r"from: Index\((\d+)\), to: Index\((\d+)\)", m).groups()
            marks.append({"at": e["timestamp"], "site": s, "kind": "A" if "Archived" in m else "P",
                          "n": int(t) - int(f) + 1, "keys": m.count("archive/repo:")})
        for e in events(svc, "StepDown", a, b):
            m = e["$metadata"]["message"]
            downs.append({"at": e["timestamp"], "site": s, "why": (re.search(r"why: (\w+)", m) or [0, "?"])[1]})
    runs = []
    for k in sorted(marks, key=lambda k: k["at"]):
        if runs and runs[-1]["site"] == k["site"] and k["at"] - runs[-1]["to"] < RUN_GAP_S * 1000:
            r = runs[-1]
        else:
            r = {"site": k["site"], "from": k["at"], "to": k["at"], "archived": 0, "objects": 0, "steps": 0,
                 "promoted": 0, "promotions": 0}
            runs.append(r)
        r["to"] = k["at"]
        if k["kind"] == "A":
            r["archived"] += k["n"]; r["objects"] += k["keys"]; r["steps"] += 1
        else:
            r["promoted"] += k["n"]; r["promotions"] += 1
    rows = []
    for r in runs:
        if not r["steps"]:
            continue
        s = max(1, round((r["to"] - r["from"]) / 1000))
        holds = [d for d in downs if r["from"] - 15_000 <= d["at"] <= r["to"] + 15_000]
        rb = row_bytes(r["from"])
        row = {"window": f"{args[w]}..{args[w + 1]}", "site": r["site"], "from": hm(r["from"]), "s": s,
               "archived": r["archived"], "objects": r["objects"], "archiveSteps": r["steps"],
               "promoted": r["promoted"], "promotions": r["promotions"],
               "archivedMB": round(r["archived"] * rb / 1e6) if rb else None, "holds": len(holds),
               "holdsPerMin": round(len(holds) / (s / 60), 2)}
        rows.append(row)
        print(json.dumps(row))
    outside = [d for d in downs if not any(r["from"] - 15_000 <= d["at"] <= r["to"] + 15_000 for r in runs)]
    h = [r["holds"] for r in rows]
    summary = {"kind": "window", "window": f"{args[w]}..{args[w + 1]}", "runs": len(rows), "holdsInRuns": sum(h),
               "holdsOutside": len(outside), "meanArchived": round(S.mean([r["archived"] for r in rows]), 1) if rows else None,
               "meanS": round(S.mean([r["s"] for r in rows]), 1) if rows else None,
               "meanHolds": round(S.mean(h), 2) if rows else None}
    for key in ("archived", "s", "promoted"):
        x = [r[key] for r in rows]
        summary[f"pearson_holds_{key}"] = pearson(x, h)
        summary[f"spearman_holds_{key}"] = pearson(rank(x), rank(h)) if rows else None
    print(json.dumps(summary))
