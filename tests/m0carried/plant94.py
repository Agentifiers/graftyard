#!/usr/bin/env python3
"""`cold-write`: planted bugs in the cold edge write's gate share and deferred learner refresh
(`workers/edge/src/colo.rs`, `gate`; `workers/edge/src/lib.rs`, `gate::key` and `may_push`;
`crates/lookup/src/lib.rs`, `refresh_config`; `decisions/DECISIONS.md`, "`cold-write`"). Plants one mutant at a time,
runs the tests that guard it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE.
Never run it under `timeout`: a kill leaves the mutant in place. Run from the repository root:

    python3 tests/m0carried/plant94.py          # every mutant
    python3 tests/m0carried/plant94.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
COLO = 'workers/edge/src/colo.rs'
EDGE = 'workers/edge/src/lib.rs'
LOOKUP = 'crates/lookup/src/lib.rs'
ET = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', '--', 'push_gate_key', 'may_push_has_an_allow',
      'push_post_serves', 'colo::tests']
LT = ['cargo', 'test', '-q', '-p', 'graftyard-lookup', '--lib', '--', 'learner_refresh']

MUTANTS = [
    ("a write keys its gate entry as a fetch does (a fetch's Allow serves a POST)", EDGE,
     '        Some(if push { push_key(k) } else { k })', '        Some(k)', ET),
    ("the push key drops the credential (another token's Allow serves a POST)", EDGE,
     '                hash.update([1]);\n                hash.update(c);', '                hash.update([1]);', ET),
    ("may_push drops the token's own reach (a contents: read token's Allow serves a POST)", EDGE,
     '        && graftyard_identity::token_allows(fields, &[("contents", Access::Write)], access) == Allow',
     '        && true', ET),
    ("may_push drops the deleted check", EDGE,
     '    graftyard_git::tiers::deleted(core).is_none()\n', '    true\n', ET),
    ("may_push asks for read, not write (a Read grant's Allow serves a POST)", EDGE,
     '        && graftyard_identity::authorize_write(p, WriteAction::Push, folds, access) == Allow',
     '        && true', ET),
    ("a push gate keeps a read-only caller's Allow", COLO,
     '            let keep = d != Decision::Allow || !share.push || fx.writable();',
     '            let keep = true;', ET),
    ("an advertisement served from its isolate's own read does not put (a POST elsewhere reads fresh)", COLO,
     '        if share.push && share.puts && d == Decision::Allow && !foreign {',
     '        if false && share.push && share.puts && d == Decision::Allow && !foreign {', ET),
    ("an advertisement serves a copy a POST took from the data center", COLO,
     '        && !(share.push && !share.matches && foreign)\n', '\n', ET),
    ("a POST puts its fresh Allow", COLO,
     'pub(crate) const POST: Share = Share { matches: true, puts: false, push: true };',
     'pub(crate) const POST: Share = Share { matches: true, puts: true, push: true };', ET),
    ("an advertisement serves from the data center's entry (it sends refs on another isolate's read)", COLO,
     'pub(crate) const ADVERT: Share = Share { matches: false, puts: true, push: true };',
     'pub(crate) const ADVERT: Share = Share { matches: true, puts: true, push: true };', ET),
    ("a POST re-reads ahead", COLO,
     '    let ahead_ok = share == READ;', '    let ahead_ok = share.matches;', ET),
    ("a deferred learner refresh is still awaited inside the read", LOOKUP,
     '    if DEFER_REFRESH.with(|d| d.get()) {', '    if DEFER_REFRESH.with(|d| d.get()) && false {', LT),
    ("a refresh dropped on a full queue stays marked (the isolate never asks again)", LOOKUP,
     '            } else {\n                due.undo();\n            }', '            } else {\n            }', LT),
    ("a failed refresh does not unmark its index", LOOKUP,
     '                e.config_index = self.had;', '                let _ = self.had;', LT),
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
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, errors='replace', timeout=1800, stdin=subprocess.DEVNULL)
            out = r.stdout + r.stderr
            if r.returncode == 0:
                verdict = 'MISSED'
            elif 'error[E' in out or 'could not compile' in out or '[build failed]' in out or 'vet:' in out:
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
