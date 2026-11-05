#!/usr/bin/env python3
"""sum2.py NAME... : one line per run. ok/sent, refusals, leader round p50/p99 (append to apply on the leader), entry
age at apply p99 on each follower, apply step ms max on every member, entries, step-downs during the run."""
import json, os, sys

B = os.environ.get("BF20_DIR", "/tmp/bf20")


def p(a, q):
    a = sorted(a)
    return a[min(len(a) - 1, int(len(a) * q))] if a else None


def load(n):
    try:
        v = json.load(open(f"{B}/raw/{n}.load.json")).get("v", {})
        return v if isinstance(v, dict) else {}
    except Exception:  # noqa: BLE001
        return {}


print("run ok/sent refused leader entries cmdsPerEntry roundP50 roundP99 followerAgeP99 stepMsMax stepdowns cmdP99")
for n in sys.argv[1:]:
    L = load(n)
    st = [json.load(open(f"{B}/raw/{n}.stats{s}.json"))["v"] for s in range(3)]
    t0 = min(x.get("alarmLast") or 0 for x in st)
    lead = [x for x in st if x.get("roundMs")]
    ld = max(lead, key=lambda x: len(x["roundMs"])) if lead else None
    fol = [x for x in st if x is not ld and x.get("applyMs")]
    downs = sum(1 for x in st for a in x.get("alarms", []) if "StepDown" in a and x.get("applySteps"))
    steps = [s[0] for x in st for s in x.get("applySteps", [])]
    ents = sum(s[1] for s in ld["applySteps"]) if ld else 0
    evs = sum(s[2] for s in ld["applySteps"]) if ld else 0
    lat = L.get("latMs", [])
    print(n, f"{L.get('ok')}/{L.get('sent')}", sum((L.get("refused") or {}).values()),
          ld["site"] if ld else None, ents, round(evs / ents) if ents else None,
          p(ld["roundMs"], .5) if ld else None, p(ld["roundMs"], .99) if ld else None,
          [p(x["applyMs"], .99) for x in fol], max(steps or [None]) if steps else None, downs, p(lat, .99))
