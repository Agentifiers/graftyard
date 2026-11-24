#!/usr/bin/env python3
"""b034-tail's two levers: planted bugs in `beside` (a cold isolate's guessed repository, and the read gate's rows
read beside the front: `workers/edge/src/lib.rs` `link`, `workers/edge/src/colo.rs` `fresh` and `link_url`) and in
`head-etags` (a chain bucket's head records each pack's etag, size and count: `crates/git/src/compose.rs`
`HeadObject` and `check_with_sets`, `crates/git/src/host.rs` `composed_answer`; `decisions/DECISIONS.md`, "`beside`"
and "`head-etags`"). Plants one mutant at a time, runs the tests that guard it, and restores the file byte for byte.
Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Never run it under `timeout`: a kill leaves the mutant in place.
Run from the repository root:

    python3 tests/m0carried/plant81.py          # every mutant
    python3 tests/m0carried/plant81.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LIB = 'workers/edge/src/lib.rs'
COLO = 'workers/edge/src/colo.rs'
COMPOSE = 'crates/git/src/compose.rs'
HOST = 'crates/git/src/host.rs'
ED = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', '--', 'beside', 'link', 'colo::', 'resolve_starts']
GIT = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'shard', '--', 'a_header_set_answers_only']
HEAD = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--lib', '--', 'a_heads_pack_records']
BED = ['bash', 'tests/composed/edgebed/run.sh']

MUTANTS = [
    # beside
    ("a guessed read is kept when the resolve named another repository or record", LIB,
     '    if a != g {\n        drop(started);', '    if false {\n        drop(started);', ED),
    ("a guess is served as the answer when the fresh read fails", LIB,
     '    let Some(a) = a? else { return Ok(None) };\n    if a != g {',
     '    let Some(a) = a.unwrap_or(Some(g.clone())) else { return Ok(None) };\n    if a != g {', ED),
    ("the decision is stamped at the gate, after the front, not at the rows' read", COLO,
     'Ok((Decision::Allow, Some(at.min(start))))', 'Ok((Decision::Allow, Some(at)))', ED),
    ("a refusal on the rows read beside is not read again fresh", COLO,
     '        Some((start, Ok((Decision::Allow, Some(at))))) => Ok((Decision::Allow, Some(at.min(start)))),',
     '        Some((start, Ok((d, Some(at))))) => Ok((d, Some(at.min(start)))),', ED),
    ("a link entry answers for a login the fresh read no longer finds", LIB,
     '    let Some(a) = a? else { return Ok(None) };\n    if a != g {',
     '    let Some(a) = a?.or(Some(g.clone())) else { return Ok(None) };\n    if a != g {', ED),
    ("a link key drops the login", COLO, '    h.update(login.as_str().as_bytes());', '    let _ = login;', ED),
    # head-etags
    ("a pack this step put again carries the old head's record (another etag)", COMPOSE,
     '            if names.len() != o.packs.len() || put.iter().any(|f| f == &format!("{name}.pack")) {',
     '            if names.len() != o.packs.len() {', HEAD),
    ("a head's record answers at another count than the manifest's", COMPOSE,
     '                    None => named_by_head.get(&name).filter(at_count).map(|h| (h.clone(), Src::Head)),',
     '                    None => named_by_head.get(&name).map(|h| (h.clone(), Src::Head)),', GIT),
    ("a pack outside the head is served from a head record", COMPOSE,
     '                let name = name_of(&p.key)?;',
     '                let name = name_of(&p.key).or_else(|| files.first().cloned())?;', GIT),
    ("a body is read without `etag_matches`", HOST,
     '                let get = bucket.get(&p.key).range(range).only_if(only).execute();',
     '                let _ = only;\n                let get = bucket.get(&p.key).range(range).execute();', BED),
]


def main():
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
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
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=1800, stdin=subprocess.DEVNULL)
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
