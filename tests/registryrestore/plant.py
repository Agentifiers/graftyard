#!/usr/bin/env python3
"""registry-restore: planted bugs in the registry row's restore after a re-placement (`graftyard_types::RowPut`).
Plants one mutant at a time, runs the tests that gate it, and restores the file byte for byte. Each line prints
CAUGHT, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/registryrestore/plant.py          # every mutant
    python3 tests/registryrestore/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
REG = 'crates/types/src/registry.rs'
T = ['-p', 'graftyard-types', '--lib', 'registry']

MUTANTS = [
    ('a restore replaces whatever row it finds', REG, '            Some(c) => self.replacing.contains(&c.do_name),',
     '            Some(_) => true,', T),
    ('a restore obeys the generation rule, as main did', REG, '            Some(c) => self.replacing.contains(&c.do_name),',
     '            Some(c) => self.row.generation >= c.generation,', T),
    ('a member\'s own write may move the row back', REG,
     '            _ if self.replacing.is_empty() => current.is_none_or(|c| self.row.generation >= c.generation),',
     '            _ if self.replacing.is_empty() => true,', T),
    ('a restore writes a row where none is stored', REG, '            None => false,', '            None => true,', T),
    ('the kept member is among the rows replaced', REG, 'tried.iter().filter(|t| t.do_name != kept.do_name)', 'tried.iter()', T),
    ('a restore with nothing to replace is still sent', REG, '        (!replacing.is_empty()).then(|| RowPut {',
     '        Some(RowPut {', T),
    ('the restored row loses its sites', REG, '                sites: sites.iter().copied().collect(),',
     '                sites: SmallVec::new(),', T),
    ('the restored row takes the newest try\'s generation', REG, '                generation: kept.generation,',
     '                generation: tried.iter().map(|t| t.generation).max().unwrap_or(kept.generation),', T),
]

ENV = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin') + ':' + os.environ['PATH'], CARGO_INCREMENTAL='0')


def run(t):
    r = subprocess.run(['cargo', 'test', '-q'] + t, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=1200)
    out = r.stdout + r.stderr
    return r.returncode, out, 'error[E' in out or 'could not compile' in out


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, t = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i:2} NOT FOUND  {name}')
            continue
        open(p, 'w').write(s.replace(old, new))
        try:
            code, out, broke = run(t)
        finally:
            open(p, 'wb').write(orig)
        if broke:
            print(f'{i:2} DID NOT COMPILE  {name}')
        elif code != 0:
            caught += 1
            fails = [l.split()[1] for l in out.splitlines() if l.startswith('---- ')]
            print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]')
        else:
            print(f'{i:2} MISSED  {name}')
    print(f'{caught} of {len(pick)} caught')


if __name__ == '__main__':
    main()
