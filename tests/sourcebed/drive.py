#!/usr/bin/env python3
"""sourcebed's calls (`run.sh`), through the driver's `/call` route. One step per run:

  drive.py <port> <genesis hex> release   the Release group from the signed genesis, then tokens:7's and tokens:8's
                                          site-0 members, each joined with the genesis's Tokens record
  drive.py <port> <genesis hex> same      tokens:7's site-1 member, joined with the same record: exit 0 if it joins
  drive.py <port> <genesis hex> other     tokens:8's site-1 member, joined with `at` moved by 1 ms: exit 0 if refused

The records are the genesis's own: a join sent with `at` 0 on a chain no member holds is refused with the source's
record (`notTheGenesisRecord` for Release, `notTheSourcesRecord` for Tokens), which the next join sends.
"""
import json
import os
import sys
import time
import urllib.request

port, ghex, step = sys.argv[1], sys.argv[2], sys.argv[3]
STATE = os.environ.get("SOURCEBED_STATE", "/tmp/sourcebed-records.json")
A = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"


def call(site, name, path, body=None):
    req = urllib.request.Request(f"http://127.0.0.1:{port}/call", method="POST",
                                 data=json.dumps({"site": site, "name": name, "path": path, "body": body}).encode())
    return json.load(urllib.request.urlopen(req, timeout=90))


def rid():
    v = int(time.time() * 1000) << 80 | int.from_bytes(os.urandom(10), "big")
    return "".join(A[(v >> (5 * i)) & 31] for i in range(25, -1, -1))


def member(chain, site):
    return {"site": site, "kind": "full", "slot": site, "generation": 1, "do_name": f"{chain}#1#f{site}"}


def join(chain, site, record, **extra):
    m = member(chain, site)
    return call(site, m["do_name"], "/join", {"chain": chain, "me": m, "record": record, **extra})


ZERO = {"name": "Tri", "sites": [0, 1, 2], "home": 0, "generation": 1, "moving_from": None, "at": 0}

if step == "release":
    r = join("release", 0, ZERO, genesis=ghex)
    rel = (r["body"].get("refused") or {}).get("notTheGenesisRecord") if isinstance(r["body"], dict) else None
    if rel is None:
        sys.exit(f"no genesis record from the Release member's refusal: {r}")
    for s in (0, 1, 2):
        r = join("release", s, rel, genesis=ghex)
        print(f"release join {s}: {r['status']}")
        if r["status"] != 200:
            sys.exit(f"release member {s} did not join: {r}")
    for _ in range(50):
        call(0, "release#1#f0", "/campaign", {})
        if call(0, "release#1#f0", "/status")["body"].get("role") == "leader":
            break
        time.sleep(0.2)
    r = call(0, "release#1#f0", "/commit", {"rid": rid(), "cmd": ghex})
    print(f"genesis commit: {r['status']} {json.dumps(r['body'])[:160]}")
    if r["status"] != 200 or "ok" not in (r["body"].get("done") or r["body"]):
        sys.exit("the genesis did not commit")
    r = join("tokens:9", 0, ZERO)
    tok = (r["body"].get("refused") or {}).get("notTheSourcesRecord") if isinstance(r["body"], dict) else None
    if tok is None:
        sys.exit(f"no Tokens record from tokens:9's refusal: {r}")
    print(f"the genesis's Tokens record: {json.dumps(tok)}")
    json.dump(tok, open(STATE, "w"))
    for c in ("tokens:7", "tokens:8"):
        r = join(c, 0, tok)
        print(f"{c} site 0 join: {r['status']}")
        if r["status"] != 200:
            sys.exit(f"{c}'s site-0 member did not join: {r}")
elif step in ("same", "other"):
    tok = json.load(open(STATE))
    chain = "tokens:7" if step == "same" else "tokens:8"
    rec = dict(tok, at=tok["at"] + (step == "other"))
    # The join is the first call this isolate takes: no read or status first, which could warm its view.
    r = join(chain, 1, rec)
    print(f"{chain} site 1 join on a cold isolate, record at {rec['at']}: {r['status']} {json.dumps(r['body'])[:300]}")
    joined = r["status"] == 200
    sys.exit(0 if joined == (step == "same") else 1)
else:
    sys.exit(f"unknown step {step}")
