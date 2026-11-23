#!/usr/bin/env python3
"""merge-front: planted bugs in a REST merge's identity reads (`decisions/DECISIONS.md`, "`merge-front`"): a merge's
reads run alone, outside their key's write-class batch (`workers/edge/src/rcache.rs`, `batched` with `alone`), and it
guesses its repository from its isolate's entries at any age (`workers/edge/src/lib.rs`, `held_repo`). Plants one
mutant at a time, runs the tests that guard it, and restores the file byte for byte. Each line prints CAUGHT, MISSED,
or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant103.py          # every mutant
    python3 tests/m0carried/plant103.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
RC = 'workers/edge/src/rcache.rs'
LIB = 'workers/edge/src/lib.rs'
TEST = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--', 'rcache', 'a_merge_guesses']
ALONE = '    if alone {\n        let answer = read.await;\n'

MUTANTS = [
    ('a call alone still joins the batch and waits', RC, ALONE,
     '    if alone && at.0 == u64::MAX {\n        let answer = read.await;\n', TEST),
    ('a call alone takes the held entry in place of its read', RC, ALONE,
     '    if alone {\n        if let Some(v) = get(cache, &k, at, true) {\n'
     '            return Batched { answer: Ok(Some(v)), shared: true, waited_ms: 0.0 };\n        }\n'
     '        let answer = read.await;\n', TEST),
    ('a call alone takes the last batched answer, read before it came', RC, ALONE,
     '    if alone {\n        let last = cache.with(|c| c.borrow().flights.get(&k).and_then(|f| f.done.clone()));\n'
     '        if let Some((_, r)) = last {\n'
     '            return Batched { answer: r.map_err(E::from), shared: true, waited_ms: 0.0 };\n        }\n'
     '        let answer = read.await;\n', TEST),
    ('a call alone does not refill the entry', RC,
     '        let answer = read.await;\n        if let Ok(Some(v)) = &answer {\n            put(cache, k, at, v.clone());\n        }\n        return Batched { answer, shared: false, waited_ms: 0.0 };\n',
     '        let answer = read.await;\n        let _ = at;\n        return Batched { answer, shared: false, waited_ms: 0.0 };\n', TEST),
    ('the merge guess needs an Authz entry within READ_CACHE, as resolve_cached did', LIB,
     '    let (id, _) = graftyard_identity::AuthzRead::resolve(&rcache::last(&rcache::AUTHZ, &account)?, &name.key())?;\n',
     '    let (id, _) = graftyard_identity::AuthzRead::resolve(&rcache::get(&rcache::AUTHZ, &account, now(), true)?, &name.key())?;\n',
     TEST),
    ('the merge guess names the repository on another record', LIB,
     '    Some(Repo { id, owner: account, record })\n}\n',
     '    Some(Repo { id, owner: account, record: PlacementRecord { generation: record.generation + 1, ..record } })\n}\n',
     TEST),
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
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=900)
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
