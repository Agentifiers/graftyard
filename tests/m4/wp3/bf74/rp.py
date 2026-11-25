#!/usr/bin/env python3
"""BF-74's Python relying party (`build/BUILD-PLAN.md`, M4-WP3): verifies an Actions ID token the way a stock Python
client does, with `urllib.request.urlopen` and no headers of its own, so it sends `User-Agent: Python-urllib/3.x`,
the agent Browser Integrity Check refuses (`decisions/OPEN-QUESTIONS.md`, F130). Discovery starts from the token's
own `iss`, so it follows the host the install names, not one the driver picks. RS256 is checked with the standard
library alone (RFC 8017 PKCS #1 v1.5 over SHA-256), so nothing here sets an agent or a TLS option.

    python3 tests/m4/wp3/bf74/rp.py <token file> [--iss https://HOST/_services/token] [--aud AUD] [--want CLAIM=VALUE]...
        [--agent AGENT]

Prints one JSON line: `fetch` (status and agent of each GET), `verified`, and the claims a policy reads. The token is
never printed. Exit 0 only when every fetch answered 200, the signature verifies under the JWKS key named by `kid`,
`iss` is `--iss` when given, `aud` is `--aud` when given, the token is unexpired, and each `--want` claim equals its
value: a trust policy's conditions, which BF-73 (`tests/m4/wp3/bf73/claims.mjs`) passes. `--agent` replaces urllib's
agent, only for BF-73's runs on a workers.dev host, whose Browser Integrity Check refuses the default (F130); BF-74
never passes it.
"""
import base64
import hashlib
import json
import sys
import time
import urllib.error
import urllib.request

# DigestInfo prefix for SHA-256 (RFC 8017, section 9.2, note 1).
SHA256_PREFIX = bytes.fromhex("3031300d060960864801650304020105000420")


def b64(s):
    return base64.urlsafe_b64decode(s + "=" * (-len(s) % 4))


def get(url, log, agent=None):
    """A GET exactly as urllib's defaults send it (or with `agent`); records the status and the agent sent."""
    req = urllib.request.Request(url, headers={"User-Agent": agent} if agent else {})
    try:
        with urllib.request.urlopen(req, timeout=20) as r:
            body, status = r.read(), r.status
    except urllib.error.HTTPError as e:
        body, status = e.read(), e.code
    agent = req.get_header("User-agent") or urllib.request.build_opener().addheaders[0][1]
    log.append({"url": url, "status": status, "agent": agent, "body": body[:80].decode("utf-8", "replace") if status != 200 else None})
    if status != 200:
        print(json.dumps({"fetch": log, "verified": False}))
        sys.exit(1)
    return json.loads(body)


def rs256_ok(n, e, signing_input, sig):
    k = (n.bit_length() + 7) // 8
    if len(sig) != k:
        return False
    em = pow(int.from_bytes(sig, "big"), e, n).to_bytes(k, "big")
    t = SHA256_PREFIX + hashlib.sha256(signing_input).digest()
    want = b"\x00\x01" + b"\xff" * (k - len(t) - 3) + b"\x00" + t
    return em == want


def main():
    args = sys.argv[1:]
    opt = lambda f: args[args.index(f) + 1] if f in args else None
    token = open(args[0]).read().strip()
    h, p, s = token.split(".")
    head, claims = json.loads(b64(h)), json.loads(b64(p))
    log = []
    agent = opt("--agent")
    disc = get(claims["iss"] + "/.well-known/openid-configuration", log, agent)
    keys = {k["kid"]: k for k in get(disc["jwks_uri"], log, agent)["keys"]}
    want = [args[i + 1].split("=", 1) for i, a in enumerate(args) if a == "--want"]
    mismatch = {k: {"want": v, "got": claims.get(k)} for k, v in want if claims.get(k) != v}
    k = keys.get(head.get("kid"))
    ok = (
        head.get("alg") == "RS256"
        and k is not None
        and disc.get("issuer") == claims["iss"]
        and rs256_ok(int.from_bytes(b64(k["n"]), "big"), int.from_bytes(b64(k["e"]), "big"), f"{h}.{p}".encode(), b64(s))
        and claims["exp"] > time.time()
        and (opt("--iss") is None or claims["iss"] == opt("--iss"))
        and (opt("--aud") is None or claims["aud"] == opt("--aud"))
    )
    accepted = ok and not mismatch
    keep = ("iss", "aud", "sub", "actor", "actor_id", "run_number", "head_ref", "base_ref", "event_name", "ref",
            "job_workflow_ref")
    print(json.dumps({"fetch": log, "verified": ok, "mismatch": mismatch, "accepted": accepted, "kid": head.get("kid"),
                      "claims": {c: claims.get(c) for c in keep}}))
    sys.exit(0 if accepted else 1)


if __name__ == "__main__":
    main()
