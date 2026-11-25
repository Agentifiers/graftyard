#!/usr/bin/env python3
"""BF-73 and BF-74's judge (`tests/m4/wp3/bf74/rp.py`): the Python relying party both hosted rows pass or fail on. It
must refuse a token whose signature, issuer, audience or expiry is wrong, refuse a verified token a trust policy's
`--want` does not match, and fetch with urllib's own agent, the one Browser Integrity Check refuses (F130), so a pass
on the custom host means what BF-74 says. Each mutant weakens one of those, and `rp_test.py` must catch it. Run from the
repository root:

    python3 tests/m0carried/plant250.py          # every mutant
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

P = 'tests/m4/wp3/bf74/rp.py'
T = ['python3', 'tests/m4/wp3/bf74/rp_test.py']
MUTANTS = [
    ("the signature is not checked", P, '        and rs256_ok(int.from_bytes(b64(k["n"]), "big")',
     '        and True or rs256_ok(int.from_bytes(b64(k["n"]), "big")', T),
    ("any issuer is taken", P, '        and (opt("--iss") is None or claims["iss"] == opt("--iss"))\n',
     '        and True\n', T),
    ("any audience is taken", P, '        and (opt("--aud") is None or claims["aud"] == opt("--aud"))\n',
     '        and True\n', T),
    ("an expired token is taken", P, '        and claims["exp"] > time.time()\n', '        and True\n', T),
    ("a policy mismatch is accepted", P, '    accepted = ok and not mismatch\n', '    accepted = ok\n', T),
    ("the fetch names its own agent", P, 'headers={"User-Agent": agent} if agent else {}',
     'headers={"User-Agent": agent or "bf74-rp"}', T),
    ("a refused fetch is read as a pass", P, '    if status != 200:\n        print(json.dumps({"fetch": log, "verified": False}))\n        sys.exit(1)\n',
     '    if status not in (200, 403):\n        print(json.dumps({"fetch": log, "verified": False}))\n        sys.exit(1)\n', T),
]

if __name__ == '__main__':
    run(MUTANTS)
