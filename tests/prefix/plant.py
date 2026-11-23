#!/usr/bin/env python3
"""exact-prefix: planted bugs in the bounded fold `prefix` (`crates/fold/src/store.rs`, `tier.rs`) and the bounded
`lease_exclusive` read (`crates/git/src/lease.rs`). Plants one mutant at a time, runs the tests that gate it, and
restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/prefix/plant.py          # every mutant
    python3 tests/prefix/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
STORE = 'crates/fold/src/store.rs'
TIER = 'crates/fold/src/tier.rs'
LEASE = 'crates/git/src/lease.rs'
PUSH = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'lease_gates', 'a_push_reads_only']
FOLD = ['cargo', 'test', '-q', '-p', 'graftyard-fold', '--lib', 'prefix_is_the_scanned_rows']
LEASES = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'lease_gates', 'an_exclusive_acquire_finds_every']

MUTANTS = [
    ('a committed store read as a fold (`Read` over `Store`) batch-scans its prefix again', TIER,
     '        Store::prefix(self, table, prefix).into_iter().map(|(k, s)| (k, s.cols)).collect()',
     '        crate::store::prefix_by_scan(prefix, |f, n| Read::scan(self, table, f, n))', PUSH),
    ('`apply`\'s writer batch-scans its prefix again', TIER,
     '        self.store.prefix(table, prefix).into_iter().map(|(k, s)| (k, s.cols)).collect()',
     '        crate::store::prefix_by_scan(prefix, |f, n| Read::scan(self, table, f, n))', PUSH),
    ('`decide`\'s overlay batch-scans its prefix again (the restored 256-row read)', STORE,
     '        let mut merged: BTreeMap<Key, Vec<Val>> = self.base.prefix(table, prefix).into_iter().collect();',
     '        let mut merged: BTreeMap<Key, Vec<Val>> = prefix_by_scan(prefix, |f, n| self.base.scan(table, f, n)).into_iter().collect();',
     PUSH),
    ('a boxed tier drops its store\'s own prefix', STORE,
     '    fn prefix(&self, table: &str, prefix: &[Val]) -> Vec<(Key, Stored)> {\n        (**self).prefix(table, prefix)\n    }\n',
     '', PUSH),
    ('the overlay ignores its deletes under the prefix', STORE,
     '                None => merged.remove(k),\n            };\n        }\n        merged.into_iter().collect()',
     '                None => None,\n            };\n        }\n        merged.into_iter().collect()', FOLD),
    ('the overlay takes its writes past the prefix', STORE,
     ".take_while(|((t, k), _)| t == table && k.starts_with(prefix))",
     ".take_while(|((t, _), _)| t == table)", FOLD),
    ('MemStore\'s prefix stops at its first key instead of the prefix\'s run', STORE,
     "        t.range(prefix.to_vec()..)\n            .take_while(|(k, _)| k.starts_with(prefix))",
     "        t.range(prefix.to_vec()..)\n            .take(1)\n            .take_while(|(k, _)| k.starts_with(prefix))", FOLD),
    ('the bounded lease_exclusive read stops after its first batch', LEASE,
     '                if !full {\n                    break;\n                }\n            }',
     '                let _ = full;\n                break;\n            }', LEASES),
]

ENV = dict(os.environ, CARGO_INCREMENTAL='0')


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, cmd = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i:2} NOT FOUND  {name}')
            continue
        # The source is never truncated in place: the original stays a hard link (no new bytes on disk), the mutant goes
        # in by rename, and the original comes back by rename. A full disk then fails the mutant's write with the source
        # untouched, instead of leaving it empty (the lead's merge run, 2026-11-23).
        keep, tmp = p + '.plant-orig', p + '.plant-new'
        os.link(p, keep)
        try:
            with open(tmp, 'w') as t:
                t.write(s.replace(old, new))
            os.replace(tmp, p)
            r = subprocess.run(cmd, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=1800)
        finally:
            if os.path.exists(tmp):
                os.remove(tmp)
            os.replace(keep, p)
            if open(p, 'rb').read() != orig:
                sys.exit(f'{f} did not restore: run `git diff -- {f}` before anything else')
        out = r.stdout + r.stderr
        if 'error[E' in out or 'could not compile' in out:
            print(f'{i:2} DID NOT COMPILE  {name}')
        elif r.returncode != 0:
            caught += 1
            print(f'{i:2} CAUGHT  {name}')
        else:
            print(f'{i:2} MISSED  {name}')
        sys.stdout.flush()
    print(f'{caught} of {len(list(pick))} caught')


if __name__ == '__main__':
    main()
