#!/usr/bin/env python3
"""BF-74's judge, `rp.py`, against a local issuer: it must accept only a token signed by the published key, naming the
issuer it was fetched from, unexpired, and matching every `--want`; and it must send urllib's own agent unless told
otherwise, since that agent is what BF-74 proves the custom host serves (F130). plant250 mutates it. Needs
`cryptography` to sign the test tokens. Exit 0 when every case holds.

    python3 tests/m4/wp3/bf74/rp_test.py
"""
import base64
import http.server
import json
import os
import subprocess
import sys
import tempfile
import threading
import time

from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.asymmetric import padding, rsa

RP = os.path.join(os.path.dirname(os.path.abspath(__file__)), "rp.py")
key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
pn = key.public_key().public_numbers()
e64 = lambda b: base64.urlsafe_b64encode(b).rstrip(b"=").decode()
ib = lambda i: i.to_bytes((i.bit_length() + 7) // 8, "big")
state = {"refuse": False, "agents": []}


class Issuer(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_GET(self):
        state["agents"].append(self.headers.get("User-Agent"))
        if state["refuse"]:
            self.send_response(403)
            self.end_headers()
            self.wfile.write(b"error code: 1010")
            return
        body = {"issuer": ISS, "jwks_uri": ISS + "/.well-known/jwks"} if self.path.endswith("openid-configuration") \
            else {"keys": [{"kty": "RSA", "kid": "kA", "n": e64(ib(pn.n)), "e": e64(ib(pn.e)), "alg": "RS256"}]}
        b = json.dumps(body).encode()
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b)


srv = http.server.HTTPServer(("127.0.0.1", 0), Issuer)
ISS = f"http://127.0.0.1:{srv.server_port}/_services/token"
threading.Thread(target=srv.serve_forever, daemon=True).start()


def token(claims, kid="kA", k=key):
    h = e64(json.dumps({"alg": "RS256", "kid": kid}).encode())
    p = e64(json.dumps(claims).encode())
    return f"{h}.{p}.{e64(k.sign(f'{h}.{p}'.encode(), padding.PKCS1v15(), hashes.SHA256()))}"


def rp(t, *args):
    with tempfile.NamedTemporaryFile("w", suffix=".jwt") as f:
        f.write(t)
        f.flush()
        r = subprocess.run([sys.executable, RP, f.name, *args], capture_output=True, text=True)
    return r.returncode, json.loads(r.stdout.strip().splitlines()[-1])


C = {"iss": ISS, "aud": "bf74", "exp": time.time() + 300, "sub": "repo:o/r:ref:refs/heads/main", "actor": "octocat"}
other = rsa.generate_private_key(public_exponent=65537, key_size=2048)
cases = [
    ("a good token passes", rp(token(C), "--iss", ISS, "--aud", "bf74")[0] == 0),
    ("a token naming another issuer fails", rp(token(C), "--iss", "https://elsewhere/_services/token")[0] == 1),
    ("a token for another audience fails", rp(token(C), "--aud", "sts")[0] == 1),
    ("a token signed by another key fails", rp(token(C, k=other))[0] == 1),
    ("a tampered payload fails", rp((lambda t: t.split(".")[0] + "." + e64(json.dumps({**C, "actor": "x"}).encode())
                                     + "." + t.split(".")[2])(token(C)))[0] == 1),
    ("an expired token fails", rp(token({**C, "exp": 1}))[0] == 1),
    ("a policy naming the token's values accepts", rp(token(C), "--want", "actor=octocat", "--want",
                                                     "sub=repo:o/r:ref:refs/heads/main")[0] == 0),
    ("a policy naming another actor refuses a verified token",
     (lambda r: r[0] == 1 and r[1]["verified"] and r[1]["mismatch"] == {"actor": {"want": "x", "got": "octocat"}})(
         rp(token(C), "--want", "actor=x"))),
    ("a policy naming a claim the token lacks refuses", rp(token(C), "--want", "run_number=3")[0] == 1),
]
state["agents"].clear()
rp(token(C))
cases.append(("urllib's own agent is sent", state["agents"] != [] and all(a.startswith("Python-urllib/") for a in state["agents"])))
state["refuse"] = True
code, out = rp(token(C))
cases.append(("a 403 from the issuer fails and is recorded", code == 1 and out["fetch"][0]["status"] == 403))
srv.shutdown()
for name, ok in cases:
    print(("ok   " if ok else "FAIL ") + name)
sys.exit(0 if all(ok for _, ok in cases) else 1)
