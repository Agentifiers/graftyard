#!/usr/bin/env python3
"""holds.py FROM_ISO TO_ISO -> B0-30 rerun: each archive run of the group's leader, the StepDowns on the group's own
members, and follower `/raft` wall times, lined up, one JSON line each (operator, 2026-11-15: do CommitErrors holds line
up with long archive runs, and follower calls past CALL_DEADLINE with them?).

- An archive run is consecutive `archive Archived` lines from one site less than RUN_GAP_S (30) apart.
- StepDowns come from the group's own objects only (tests/am1/raw/b030-objects.txt), each with its site, kind, target,
  and whether it fell inside an archive run.
- For each run, and for the same length of time just before it, every follower site's `/raft` p50, max and count past
  CALL_DEADLINE_MS (1000, `raft::CALL_DEADLINE`), from the group's member objects.
Token from the environment, by header only."""
import datetime as D, json, os, re, statistics as S, sys, time, urllib.error, urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
def fetch(r):
    """The logs API answers a 500 or times out a handshake now and then; retry it twice."""
    for i in range(3):
        try:
            return json.load(urllib.request.urlopen(r, timeout=60))
        except (urllib.error.URLError, TimeoutError) as e:
            if (isinstance(e, urllib.error.HTTPError) and e.code < 500) or i == 2:
                raise
            time.sleep(5 * (i + 1))


ms = lambda iso: int(D.datetime.fromisoformat(iso.replace("Z", "+00:00")).timestamp() * 1000)
iso = lambda t: D.datetime.fromtimestamp(t / 1000, D.UTC).strftime("%H:%M:%S")
a, b = ms(sys.argv[1]), ms(sys.argv[2])
RUN_GAP_S, DEADLINE = 30, int(os.environ.get("CALL_DEADLINE_MS", "1000"))
run_ids = {l.strip() for l in open("tests/am1/raw/b030-objects.txt") if l.strip()}
SITES = {"a": 0, "b": 1, "c": 2}


def events(svc, filters, fr, to):
    # The events view returns at most 100 events a query, so read in 2-minute windows.
    out, step = [], 120_000
    for t in range(fr, to, step):
        body = {"queryId": "q", "timeframe": {"from": t, "to": min(to, t + step)}, "view": "events", "limit": 100,
                "parameters": {"filters": [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": svc},
                                           *filters]}}
        r = urllib.request.Request(f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
            data=json.dumps(body).encode(), headers={"authorization": f"Bearer {tok}", "content-type": "application/json"}, method="POST")
        out += fetch(r)["result"]["events"]["events"]
    return [e for e in out if e.get("$workers", {}).get("durableObjectId") in run_ids]


msg = lambda v: [{"key": "$metadata.message", "operation": "includes", "type": "string", "value": v}]
runs, downs = [], []
for s in SITES:
    svc = f"graft-m1-site-{s}"
    ts = sorted(e["timestamp"] for e in events(svc, msg("archive Archived"), a, b))
    for t in ts:
        if runs and runs[-1]["site"] == s and t - runs[-1]["to"] < RUN_GAP_S * 1000:
            runs[-1]["to"], runs[-1]["entries"] = t, runs[-1]["entries"] + 1
        else:
            runs.append({"site": s, "from": t, "to": t, "entries": 1})
    for e in events(svc, msg("StepDown"), a, b):
        m = e["$metadata"]["message"]
        kind = re.search(r"why: (\w+)", m)
        to = re.search(r"to: (None|Some\(SiteId\((\d+)\)\))", m)
        downs.append({"at": e["timestamp"], "site": s, "kind": kind[1] if kind else "?",
                      "to": None if not to or to[1] == "None" else int(to[2])})

def calls(s, fr, to):
    """Follower site `s`'s `/raft` on the group's own member: count, p50 and max wall ms, and the count past the
    deadline, from the logs' aggregate view (the events view caps each query at 100)."""
    flt = [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": f"graft-m1-site-{s}"},
           {"key": "$metadata.message", "operation": "includes", "type": "string", "value": "member/raft"}]
    def agg(extra):
        body = {"queryId": "q", "timeframe": {"from": fr, "to": max(to, fr + 1000)}, "view": "calculations", "limit": 200,
                "parameters": {"calculations": [{"operator": "count"},
                                                {"operator": "median", "key": "$workers.wallTimeMs", "keyType": "number"},
                                                {"operator": "max", "key": "$workers.wallTimeMs", "keyType": "number"}],
                               "groupBys": [{"type": "string", "value": "$workers.durableObjectId"}], "filters": flt + extra}}
        r = urllib.request.Request(f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
            data=json.dumps(body).encode(), headers={"authorization": f"Bearer {tok}", "content-type": "application/json"}, method="POST")
        out = {}
        for c in fetch(r)["result"]["calculations"]:
            for x in c["aggregates"]:
                if x["groups"][0]["value"] in run_ids:
                    out[c["calculation"].split("(")[0]] = x["value"]
        return out
    v = agg([])
    past = agg([{"key": "$workers.wallTimeMs", "operation": "gt", "type": "number", "value": DEADLINE}]).get("count", 0)
    return {"n": v.get("count", 0), "p50": v.get("median"), "max": v.get("max"), "past": past}


for r in sorted(runs, key=lambda r: r["from"]):
    span = max(r["to"] - r["from"], 1)
    inside = [d for d in downs if r["from"] - 15_000 <= d["at"] <= r["to"] + 15_000]
    fol = [s for s in SITES if s != r["site"]]
    print(json.dumps({"kind": "run", "site": r["site"], "from": iso(r["from"]), "to": iso(r["to"]),
        "s": round(span / 1000), "entries": r["entries"], "stepDowns": len(inside),
        "during": {s: calls(s, r["from"], r["to"]) for s in fol},
        "before": {s: calls(s, r["from"] - span, r["from"]) for s in fol}}))
for d in sorted(downs, key=lambda d: d["at"]):
    within = any(r["from"] - 15_000 <= d["at"] <= r["to"] + 15_000 for r in runs)
    print(json.dumps({"kind": "stepDown", "at": iso(d["at"]), "site": d["site"], "why": d["kind"], "to": d["to"],
                      "inArchiveRun": within}))
