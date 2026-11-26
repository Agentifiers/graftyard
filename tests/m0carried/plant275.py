#!/usr/bin/env python3
"""`graftyard runners init` writes `HostedRunnersAppKey` only when `--app-key` is given and the App accepts a JWT signed
with it (`cli/src/runners.rs`, `init_with` and `app_accepts`; `decisions/DECISIONS.md`, "`init-key-guard`"). R23 M3's
r1 wrote a retired key over the current one, and every runner start failed on 401. The first mutant is main's rule: the
key is written unchecked. Run from the repository root:

    python3 tests/m0carried/plant275.py          # every mutant
    python3 tests/m0carried/plant275.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

R = 'cli/src/runners.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard', '--bin', 'graftyard', 'runners::']
MUTANTS = [
    ("the key is written without asking the App, as on main", R,
     '            read(p).and_then(|k| app_accepts(&spec, &k).map(|()| k)).map_err',
     '            read(p).map_err', T),
    ("any answer but a transport error counts as accepted", R,
     '    if status == 200 {\n        return Ok(());\n    }\n', '    if status < 600 {\n        return Ok(());\n    }\n', T),
    ("the JWT names no App", R,
     '"iss": app }).to_string());', '"iss": 0 }).to_string());', T),
    ("no --app-key and no stored key goes on to deploy", R,
     '        None if cf.has_secret(&store, APP_KEY_SECRET)? => println!', '        None if true => println!', T),
    ("no --app-key writes nothing but refuses even when a key is stored", R,
     '        None if cf.has_secret(&store, APP_KEY_SECRET)? => println!', '        None if false => println!', T),
]

if __name__ == '__main__':
    run(MUTANTS)
