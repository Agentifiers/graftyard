#!/usr/bin/env python3
"""colo-gate: planted bugs in the data-center share of the read gate's decisions and of checked chain headers
(`workers/edge/src/colo.rs`, `crates/git/src/compose.rs` `check_with_sets`; `decisions/DECISIONS.md`, "`colo-gate`").
Plants one mutant at a time, runs the tests that guard it, and restores the file byte for byte. Each line prints CAUGHT,
MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant79.py          # every mutant
    python3 tests/m0carried/plant79.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
COLO = 'workers/edge/src/colo.rs'
COMPOSE = 'crates/git/src/compose.rs'
ED = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', '--', 'colo::']
GIT = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'shard', '--', 'a_header_set_answers_only']

MUTANTS = [
    ("the key drops the credential", COLO,
     '''    match credential {
        Some(c) => {
            h.update([1]);
            h.update(c);
        }
        None => h.update([0]),
    }''', '    let _ = credential;', ED),
    ("the key drops the repository", COLO, '    h.update(repo.0.0.to_be_bytes());', '    let _ = repo;', ED),
    ("the key drops the placement record", COLO, '    h.update(serde_json::to_vec(record).unwrap_or_default());',
     '    let _ = record;', ED),
    ("a refusal is put", COLO, '            if share.puts && d == Decision::Allow && keep {\n                fx.share(at).await;',
     '            if share.puts {\n                fx.share(at).await;', ED),
    ("the stamp is the put's time, not the fold read's", COLO,
     '            if share.puts && d == Decision::Allow && keep {\n                fx.share(at).await;',
     '            if share.puts && d == Decision::Allow && keep {\n                fx.share(fx.now()).await;', ED),
    ("a shared entry serves up to READ_CACHE, with no skew", COLO,
     '&& age < ms(READ_CACHE) - ms(COLO_SKEW)', '&& age <= ms(READ_CACHE)', ED),
    ("the re-read's refusal is put", COLO,
     '        fx.hold(at, d, false);\n        if d == Decision::Allow {\n            fx.share(at).await;\n        }\n    }\n}',
     '        fx.hold(at, d, false);\n        fx.share(at).await;\n    }\n}', ED),
    ("an isolate copy restamps at copy time", COLO, '            fx.hold(at, Decision::Allow, true);',
     '            fx.hold(now, Decision::Allow, true);', ED),
    ("an isolate copy is held as its own read (no skew)", COLO, '            fx.hold(at, Decision::Allow, true);',
     '            fx.hold(at, Decision::Allow, false);', ED),
    ("the share-off arm still matches the data center", COLO, '    if share.matches\n        && let Some(at) = fx.matched().await',
     '    if true\n        && let Some(at) = fx.matched().await', ED),
    ("a chain set's key drops the head's version", COLO, '    h.update(version.as_bytes());',
     '    let _ = version;', ED),
    ("a header set answers for packs its head does not name", COMPOSE,
     '                let name = name_of(&p.key)?;', '                let name = name_of(&p.key).unwrap_or_default();', GIT),
    ("a header set ignores the manifest's count", COMPOSE,
     '                match set.packs.get(&p.key).filter(at_count) {', '                match set.packs.get(&p.key) {', GIT),
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
