#!/usr/bin/env python3
"""The lifeboat front refuses both legs of a push (`workers/edge/src/lifeboat.rs`, `serves`): planted bugs that let
the receive-pack advertisement (`GET info/refs?service=git-receive-pack`) or the push's POST past the read-only check,
where the advertisement reached `receive_pack_stream` and answered 200 with the copy's refs on main `271fedba`
(`tests/lifeboatbed/run.sh`, check 2). Plants one mutant at a time, runs the edge's lifeboat test, and restores the
file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant235.py          # every mutant
    python3 tests/m0carried/plant235.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

LB = 'workers/edge/src/lifeboat.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', 'lifeboat']

MUTANTS = [
    ("the receive-pack advertisement is served", LB,
     "    if path.ends_with(\"/info/refs\") && query.split('&').any(|kv| kv == \"service=git-receive-pack\") {\n",
     "    if path.ends_with(\"/info/refs\") && query == \"service=receive-pack\" {\n", T),
    ("only the first query pair names the service", LB,
     "query.split('&').any(|kv| kv == \"service=git-receive-pack\")",
     "query.split('&').next() == Some(\"service=git-receive-pack\")", T),
    ("the push's POST is served", LB,
     '        "POST" => path.ends_with("/git-upload-pack") || path == "/api/graphql",\n',
     '        "POST" => path.contains("/git-") || path == "/api/graphql",\n', T),
]

if __name__ == '__main__':
    run(MUTANTS)
