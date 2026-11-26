#!/usr/bin/env python3
"""F129's hosted check on graft-m4a (`decisions/OPEN-QUESTIONS.md`, F129): a relying party whose clock lags the edge's
by LAG seconds (default 60) accepts a job's ID token. `rp.py` (BF-74's relying party) checks the signature, `iss`,
`aud` and `exp` but not `nbf`, so this runs it with `time.time` set LAG seconds behind the edge's clock and then
applies the check a lagging relying party adds: `nbf` at or before its own now. The edge's clock is read from its
`Date` header, so the orb's own offset does not count toward the lag. Prints one JSON line; the token is never printed.

    python3 tests/m4/wp3/bf77/lag.py <token file> <edge url> [--lag 60] [rp.py args...]
"""
import base64
import email.utils
import json
import os
import runpy
import sys
import time
import urllib.request

args = sys.argv[1:]
token_file, edge = args[0], args[1].rstrip("/")
rest = args[2:]
lag = 60.0
if "--lag" in rest:
    i = rest.index("--lag")
    lag = float(rest[i + 1])
    del rest[i : i + 2]

req = urllib.request.Request(edge + "/status", headers={"User-Agent": "curl/8.5.0"})
t0 = time.time()
try:
    with urllib.request.urlopen(req, timeout=20) as r:
        date = r.headers["Date"]
except urllib.error.HTTPError as e:
    date = e.headers["Date"]
t1 = time.time()
edge_now = email.utils.parsedate_to_datetime(date).timestamp()
# The orb's clock minus the edge's (`Date` has whole seconds, so up to 1 s of error).
offset = (t0 + t1) / 2 - edge_now
real = time.time


def lagged():
    return real() - offset - lag


p = open(token_file).read().strip().split(".")[1]
claims = json.loads(base64.urlsafe_b64decode(p + "=" * (-len(p) % 4)))
now = lagged()
nbf_ok = claims["nbf"] <= now
line = {"lag_s": lag, "orb_minus_edge_s": round(offset, 1), "iat": claims["iat"], "nbf": claims["nbf"],
        "exp": claims["exp"], "nbf_before_iat_s": claims["iat"] - claims["nbf"], "rp_now": round(now),
        "nbf_ok": nbf_ok}
print(json.dumps(line), flush=True)
time.time = lagged
sys.argv = [os.path.join(os.path.dirname(os.path.abspath(__file__)), "../bf74/rp.py"), token_file, *rest]
code = 0
try:
    runpy.run_path(sys.argv[0], run_name="__main__")
except SystemExit as e:
    code = e.code or 0
print(json.dumps({"rp_exit": code, "accepted": code == 0 and nbf_ok}))
sys.exit(0 if code == 0 and nbf_ok else 1)
