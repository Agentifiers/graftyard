#!/usr/bin/env python3
"""push-reads: planted bugs in a push's reads (`decisions/DECISIONS.md`, "`push-reads`"). Plants one mutant at a time,
runs the test that guards it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE.
Run from the repository root:

    python3 tests/m0carried/plant23.py          # every mutant
    python3 tests/m0carried/plant23.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
FAMILY = 'workers/site/src/family.rs'
LOOKUP = 'crates/lookup/src/lib.rs'
EDGE = 'workers/edge/src/lib.rs'
SITE = ['cargo', 'test', '-q', '-p', 'graftyard-site', 'a_push_reads']
LOOK = ['cargo', 'test', '-q', '-p', 'graftyard-lookup', 'a_commit']
HOT = ['cargo', 'test', '-q', '-p', 'graftyard-edge', 'hot_tests']

MUTANTS = [
    ('the combined read answers access alone', FAMILY,
     '            core.extend(graftyard_api::hosted::TierRows::agent_rows(&fold.core));\n', '', SITE),
    ('the combined read does not say it carries agent rows', FAMILY,
     '            v["agent"] = json!(true);\n', '', SITE),
    ('an access-only read says it carries agent rows', FAMILY,
     'let both = r.get("agent").is_some();', 'let both = true;', SITE),
    ('a stale term keeps its leader', LOOKUP,
     '            Some(t) if t < e.term => e.leader = None,\n', '', LOOK),
    ('a stale term replaces the leader', LOOKUP,
     'Some(t) if t < e.term => e.leader = None,', 'Some(t) if t < e.term => e.leader = leader,', LOOK),
    ('NotLeader does not drop the leader', LOOKUP,
     '            e.leader = None;\n        }\n    });\n}', '        }\n    });\n}', LOOK),
    ('a commit never probes', LOOKUP,
     '    cached_leader(chain).is_some()\n', '    true\n', LOOK),
    ('a cool group stages first', EDGE,
     '            && STAGED.with(|m| m.borrow().get(&repo).is_some_and(|at| now.0.saturating_sub(at.0) <= w))',
     '            && STAGED.with(|m| m.borrow().get(&repo).is_none_or(|at| now.0.saturating_sub(at.0) <= w))', HOT),
    ('the hot mark never lapses', EDGE,
     'now.0.saturating_sub(at.0) <= w))', 'now.0.saturating_sub(at.0) <= u64::MAX))', HOT),
    ('a pack the leader keeps inline stages first', EDGE,
     '            || inline > INLINE_MAX_HOT\n', '            || inline >= INLINE_MAX_HOT\n', HOT),
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
