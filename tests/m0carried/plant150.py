#!/usr/bin/env python3
"""agent-rows-cache: planted bugs in the read gate's agent rows (`workers/edge/src/colo.rs`, `gate`;
`workers/edge/src/lib.rs`, `gate::put`; `workers/edge/src/agents.rs`, `unlinked_by_cache`; `decisions/DECISIONS.md`,
"`agent-rows-cache`"). A push gate's entry holds the agent rows its own fill read and serves them only with that
decision; a data-center copy carries none; and a push whose cached rows lack its own task reads them again. Plants one
mutant at a time, runs the tests, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT
COMPILE. Run from the repository root:

    python3 tests/m0carried/plant150.py          # every mutant
    python3 tests/m0carried/plant150.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
T = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', '--', 'gate', 'colo', 'agents']
C, L, A = 'workers/edge/src/colo.rs', 'workers/edge/src/lib.rs', 'workers/edge/src/agents.rs'

MUTANTS = [
    ('a push served from the isolate entry gets no rows, so it reads its own (main)', C,
     'served: Served::Isolate(now.0.saturating_sub(at.0)), ahead, rows };',
     'served: Served::Isolate(now.0.saturating_sub(at.0)), ahead, rows: None };'),
    ('a fresh read holds its rows but does not hand them to the push', C,
     '            (Ok(d), rows)\n        }\n        Ok((d, None, rows)) => (Ok(d), rows),',
     '            (Ok(d), None)\n        }\n        Ok((d, None, rows)) => (Ok(d), rows),'),
    ('a data-center copy keeps the older fill\'s rows beside its decision', C,
     '            fx.hold(at, Decision::Allow, true, None);',
     '            fx.hold(at, Decision::Allow, true, fx.held().and_then(|h| h.3));'),
    ('a fresh refusal keeps the last Allow\'s rows', C,
     '                fx.hold(at, d, false, rows.clone());',
     '                fx.hold(at, d, false, fx.held().and_then(|h| h.3).or(rows.clone()));'),
    ('a replaced entry\'s rows are never released from the byte count', L,
     '                if let Some((_, n)) = c.remove(&k).and_then(|e| e.rows) {\n                    b.set(b.get().saturating_sub(n));\n                }',
     '                c.remove(&k);'),
    ('the byte cap is ignored', L,
     '(b.get() + n <= ROWS_BYTES_MAX).then(', '(true).then('),
    ('a put without rows keeps the entry\'s earlier rows', L,
     '                if let Some((_, n)) = c.remove(&k).and_then(|e| e.rows) {\n                    b.set(b.get().saturating_sub(n));\n                }\n                let rows = rows.and_then(|r| {',
     '                let old = c.remove(&k).and_then(|e| e.rows);\n                if let Some((_, n)) = &old {\n                    b.set(b.get().saturating_sub(*n));\n                }\n                let rows = rows.or(old.map(|(r, _)| r)).and_then(|r| {'),
    ('cached rows missing the push\'s task are trusted', A,
     '    if !a.cached {\n        return false;\n    }', '    if a.cached {\n        return false;\n    }'),
    ('the token\'s own claim is not counted, so its push reads again', A,
     '    own.extend(graftyard_git::task::claim(&push.actor).cloned());\n', ''),
    ('another App\'s session counts as one the rows should list', A,
     'is_some_and(|id| mine(&id) && !own.contains(&id))', 'is_some_and(|id| !own.contains(&id))'),
]

ENV = dict(os.environ, CARGO_INCREMENTAL='0')


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, F, old, new = MUTANTS[i]
        p = os.path.join(ROOT, F)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i:2} NOT FOUND  {name}')
            continue
        # As tests/prefix/plant.py: the original stays a hard link and the mutant goes in by rename, so a full disk
        # never leaves the source empty.
        keep, tmp = p + '.plant-orig', p + '.plant-new'
        os.link(p, keep)
        try:
            with open(tmp, 'w') as t:
                t.write(s.replace(old, new))
            os.replace(tmp, p)
            r = subprocess.run(T, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=3600)
        finally:
            if os.path.exists(tmp):
                os.remove(tmp)
            os.replace(keep, p)
            if open(p, 'rb').read() != orig:
                sys.exit(f'{F} did not restore: run `git diff -- {F}` before anything else')
        out = r.stdout + r.stderr
        if 'error[E' in out or 'could not compile' in out:
            print(f'{i:2} DID NOT COMPILE  {name}')
        elif r.returncode != 0:
            caught += 1
            print(f'{i:2} CAUGHT  {name}: {next((l for l in out.splitlines() if "panicked" in l), "")[:160]}')
        else:
            print(f'{i:2} MISSED  {name}')
        sys.stdout.flush()
    print(f'{caught} of {len(list(pick))} caught')
    sys.exit(0 if caught == len(list(pick)) else 1)


if __name__ == '__main__':
    main()
