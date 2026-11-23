# wlmsg.py SERVICE NAME FROM_ISO TO_ISO: console lines and non-ok invocations of one object, with messages, 1 s pages.
import sys, json, datetime
sys.argv, args = sys.argv[:1], sys.argv[1:]
sys.path.insert(0, 'tests/fanout')
import wlobj
svc, name, a, b = args
do = wlobj.object_id(name, a, b)
t, end = wlobj.ms(a), wlobj.ms(b)
fmt = lambda m: datetime.datetime.fromtimestamp(m / 1000, datetime.UTC).strftime('%H:%M:%S.%f')[:-3]
while t < end:
    body = {"queryId": "q", "timeframe": {"from": t, "to": min(end, t + 1000)}, "view": "events", "limit": 2000,
            "parameters": {"filters": wlobj.filters(svc, do), "limit": 2000}}
    for e in sorted(wlobj.post(wlobj.URL, body)["result"].get("events", {}).get("events", []), key=lambda e: e["timestamp"]):
        w, m = e.get("$workers", {}), e.get("$metadata", {})
        if m.get("type") == "cf-worker" or w.get("outcome") not in ("ok", None):
            print(fmt(e["timestamp"]), m.get("type"), w.get("eventType"), (w.get("event") or {}).get("path"), w.get("outcome"),
                  w.get("wallTimeMs"), w.get("cpuTimeMs"), json.dumps(e.get("source"))[:400])
    t += 1000
