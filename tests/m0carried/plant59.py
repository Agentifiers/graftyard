#!/usr/bin/env python3
"""write-identity: planted bugs in which writes take their token row from the edge's cache and in how a credential
refused at `decide` renders (`decisions/DECISIONS.md`, "`write-identity`"). Plants one mutant at a time, runs the tests
that guard it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Mutants 0, 4,
5 and 7 put main's behaviour back. Run from the repository root:

    python3 tests/m0carried/plant59.py          # every mutant
    python3 tests/m0carried/plant59.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
RC = 'workers/edge/src/rcache.rs'
API = 'workers/edge/src/api.rs'
LIB = 'workers/edge/src/lib.rs'
HOSTED = 'crates/api/src/hosted.rs'
REST = 'crates/api/src/rest.rs'
EDGE = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', '--', 'a_covered_write_takes',
        'only_writes_whose_decide', 'only_a_push_s_post']
SURF = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'surfaces', 'a_revoked_tokens_write']

MUTANTS = [
    ("main's front: no write takes the cached row", RC,
     '    (decided && !read_class).then(|| get(cache, k, at, true)).flatten()',
     '    let _ = (cache, k, at, read_class, decided);\n    None', EDGE),
    ("a write decide does not cover takes the cached row", RC,
     '    (decided && !read_class).then(|| get(cache, k, at, true)).flatten()',
     '    (!read_class).then(|| get(cache, k, at, true)).flatten()', EDGE),
    ("a REST merge is decided (git cache work first)", API,
     '            | (Method::Post, ["statuses", _])\n',
     '            | (Method::Post, ["statuses", _])\n            | (Method::Put, ["pulls", _, "merge"])\n', EDGE),
    ("mergePullRequest is decided", API,
     '!matches!(f.as_str(), "mergePullRequest" | "createRepository")',
     '!matches!(f.as_str(), "createRepository")', EDGE),
    ("main's GraphQL: a decide-time 401 answers HTTP 200", HOSTED,
     '    WireReply::of(&Reply::json(if credential { 401 } else { 200 }, v))',
     '    let _ = credential;\n    WireReply::of(&Reply::json(200, v))', SURF),
    ("main's labels: a decide-time 401 answers 403", REST,
     '        E::Unauthorized(_) => Reply::error(401, "Bad credentials", "rest"),\n        E::Forbidden => Reply::error(403, "Resource not accessible by integration", LABELS_DOC),',
     '        E::Forbidden | E::Unauthorized(_) => Reply::error(403, "Resource not accessible by integration", LABELS_DOC),', SURF),
    ("a review comment's 401 keeps the route's documentation_url", REST,
     '        // The front\'s body for a refused credential, whatever the route (`write-identity`).\n        E::Unauthorized(_) => Reply::error(401, "Bad credentials", "rest"),',
     '        E::Unauthorized(_) => Reply::error(401, "Bad credentials", doc),', SURF),
    ("main's git: no push is decided", LIB,
     '    matches!(route, Route::Rpc { service: Service::ReceivePack, .. })',
     '    let _ = route;\n    false', EDGE),
    ("receive-pack's advertisement is decided (no command behind it)", LIB,
     '    matches!(route, Route::Rpc { service: Service::ReceivePack, .. })',
     '    matches!(route, Route::Rpc { service: Service::ReceivePack, .. } | Route::InfoRefs { service: Some(Service::ReceivePack), .. })', EDGE),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, cmd = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        text = orig.decode()
        if text.count(old) != 1:
            print(f'{i} {name}: anchor found {text.count(old)} times')
            sys.exit(2)
        try:
            open(p, 'w').write(text.replace(old, new))
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=1800)
            out = r.stdout + r.stderr
            if r.returncode == 0:
                verdict = 'MISSED'
            elif 'error[E' in out or 'could not compile' in out:
                verdict = 'DID NOT COMPILE'
            else:
                verdict = 'CAUGHT'
                caught += 1
        finally:
            open(p, 'wb').write(orig)
        print(f'{i} {name}: {verdict}', flush=True)
    print(f'{caught} of {len(pick)} caught')
    sys.exit(0 if caught == len(pick) else 1)


if __name__ == '__main__':
    main()
