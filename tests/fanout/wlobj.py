#!/usr/bin/env python3
"""wlobj.py: one Durable Object's Workers Logs invocations over a window, for joining with a row's member trace
(`tests/am1/results/b033-percmd.md`).

  wlobj.py fetch SERVICE NAME FROM_ISO TO_ISO OUT.json   NAME is the object's name, such as `repo:131387#1#f0`; its
                                                         id comes from durableObjectsPeriodicGroups (the id that used
                                                         the most CPU). Writes one row per invocation: timestamp,
                                                         wallTimeMs, cpuTimeMs, path and the x-graft-* headers. No
                                                         account fields. Needs CLOUDFLARE_ACCOUNT_ID and
                                                         CLOUDFLARE_API_TOKEN (sent as a header only).
  wlobj.py count SERVICE NAME FROM_ISO TO_ISO            the same window's exact invocation count (`calculations`)
                                                         against the rows `fetch` would return, to check coverage.
"""
import json, os, sys, time, urllib.request
from datetime import datetime

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
H = {"authorization": f"Bearer {tok}", "content-type": "application/json"}
URL = f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query"
ms = lambda s: int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)


def post(url, body):
    for i in range(5):
        try:
            return json.load(urllib.request.urlopen(urllib.request.Request(url, data=json.dumps(body).encode(),
                                                                           method="POST", headers=H), timeout=120))
        except Exception as e:
            print("retry", i, e, file=sys.stderr)
            time.sleep(3 + 3 * i)
    raise SystemExit("query failed")


def object_id(name, a, b):
    # Periodic groups are per minute: widen the lookup to whole minutes around the window.
    iso = lambda m: datetime.utcfromtimestamp(m / 1000).strftime("%Y-%m-%dT%H:%M:00Z")
    a, b = iso(ms(a) - 60000), iso(ms(b) + 120000)
    q = """query($a:String!,$s:Time!,$e:Time!,$n:String!){viewer{accounts(filter:{accountTag:$a}){
 p: durableObjectsPeriodicGroups(limit: 1000, filter:{datetime_geq:$s, datetime_lt:$e, name:$n}) {
  sum { cpuTime } dimensions { objectId } } }}}"""
    v = post("https://api.cloudflare.com/client/v4/graphql", {"query": q, "variables": {"a": acct, "s": a, "e": b, "n": name}})
    per = {}
    for g in v["data"]["viewer"]["accounts"][0]["p"]:
        per[g["dimensions"]["objectId"]] = per.get(g["dimensions"]["objectId"], 0) + g["sum"]["cpuTime"]
    return max(per, key=per.get)


def filters(svc, do):
    return [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": svc},
            {"key": "$workers.durableObjectId", "operation": "eq", "type": "string", "value": do}]


def fetch(svc, name, a, b, out):
    do, rows, t, end = object_id(name, a, b), [], ms(a), ms(b)
    while t < end:
        body = {"queryId": "q", "timeframe": {"from": t, "to": min(end, t + 1000)}, "view": "events", "limit": 2000,
                "parameters": {"filters": filters(svc, do), "limit": 2000}}
        ev = post(URL, body)["result"].get("events", {}).get("events", [])
        if len(ev) >= 2000:
            print("chunk full at", t, file=sys.stderr)
        for e in ev:
            w = e.get("$workers", {})
            req = (w.get("event") or {}).get("request") or {}
            h = req.get("headers", {}) if isinstance(req, dict) else {}
            rows.append({"ts": e["timestamp"], "wall": w.get("wallTimeMs"), "cpu": w.get("cpuTimeMs"),
                         "path": (w.get("event") or {}).get("path"), "outcome": w.get("outcome"),
                         **{k[8:]: v for k, v in h.items() if k.startswith("x-graft-")}})
        t += 1000
    json.dump(rows, open(out, "w"))
    print(name, len(rows), "invocations")


def count(svc, name, a, b):
    do = object_id(name, a, b)
    body = {"queryId": "q", "timeframe": {"from": ms(a), "to": ms(b)}, "view": "calculations", "limit": 10,
            "parameters": {"filters": filters(svc, do), "calculations": [{"operator": "count"},
                                                                          {"operator": "sum", "key": "$workers.cpuTimeMs",
                                                                           "keyType": "number"}]}}
    c = post(URL, body)["result"]["calculations"]
    print(name, "count", c[0]["aggregates"][0]["value"] if c[0]["aggregates"] else 0,
          "cpu ms", c[1]["aggregates"][0]["value"] if c[1]["aggregates"] else 0)


if __name__ == "__main__":
    {"fetch": lambda: fetch(*sys.argv[2:7]), "count": lambda: count(*sys.argv[2:6])}[sys.argv[1]]()
