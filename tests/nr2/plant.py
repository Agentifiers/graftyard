#!/usr/bin/env python3
"""narrow-reads-2: planted bugs in the edge's per-isolate read caches (`workers/edge/src/rcache.rs`). Plants one mutant
at a time, runs the edge's tests, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT
COMPILE. The edge's wiring (a name the cached fold lacks resolved fresh, a gate the cached fold closes read again) is
covered hosted by `tests/nr2/stale.mjs`. Run from the worktree root:

    python3 tests/nr2/plant.py          # every mutant
    python3 tests/nr2/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
RC = 'workers/edge/src/rcache.rs'
EDGE = ('graftyard-edge', '--lib', 'rcache')

MUTANTS = [
    ('a write-class request reads the cache', RC,
     '        if !read_class {\n            return None;\n        }', '        let _ = read_class;', EDGE),
    ('the TTL is ignored (a revocation is never seen)', RC,
     '.filter(|(at, _)| now.0.saturating_sub(at.0) <= ttl)', '.filter(|_| ttl > 0)', EDGE),
    ('the TTL is one second too long', RC,
     'now.0.saturating_sub(at.0) <= ttl)', 'now.0.saturating_sub(at.0) <= ttl + 1_000)', EDGE),
    ('an older answer replaces a newer one', RC,
     '|| self.entries.get(&k).is_some_and(|(at, _)| *at > read_at)', '|| false', EDGE),
    ('every answer is kept, a revocation too', RC,
     '        if !(self.keep)(&v) || ', '        if ', EDGE),
    ('a revoked token row is kept', RC,
     'fn usable_row(r: &crate::TokenRow) -> bool {\n    !r.revoked', 'fn usable_row(r: &crate::TokenRow) -> bool {\n    let _ = r;\n    true', EDGE),
    ('a revoked jti is kept', RC,
     'fn not_revoked(revoked: &bool) -> bool {\n    !*revoked', 'fn not_revoked(revoked: &bool) -> bool {\n    let _ = revoked;\n    true', EDGE),
    ('through: a write-class request takes the cached entry', RC,
     '    if read_class {\n        if let Some(v) = get(cache, &k, at, true) {',
     '    if let Some(v) = get(cache, &k, at, true) {\n        return Ok(Some(v));\n    }\n    if read_class {\n        if let Some(v) = get(cache, &k, at, true) {', EDGE),
    ('through: a write\'s fresh answer does not refresh the entry', RC,
     '    running.end(shared);\n    if let Ok(Some(v)) = &answer {\n        put(cache, k, at, v.clone());\n    }',
     '    running.end(shared);\n    let _ = &at;', EDGE),
    ('an entry is stamped when its answer arrives, not when its read started', RC,
     '    running.end(shared);\n    if let Ok(Some(v)) = &answer {\n        put(cache, k, at, v.clone());',
     '    running.end(shared);\n    if let Ok(Some(v)) = &answer {\n        put(cache, k, Timestamp(at.0 + 4_000), v.clone());', EDGE),
    ('past the cap, the map keeps growing', RC,
     '        if self.entries.len() >= self.cap && !self.entries.contains_key(&k) {', '        if false {', EDGE),
    ('past the cap, an update of a held key clears the map', RC,
     '        if self.entries.len() >= self.cap && !self.entries.contains_key(&k) {', '        if self.entries.len() >= self.cap {', EDGE),
]

ENV = dict(os.environ, CARGO_INCREMENTAL='0',
           CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, 'target', 'plants')))


def run(t):
    pkg, kind, name = t
    args = ['cargo', 'test', '-q', '-p', pkg, kind, name] if kind == '--test' else ['cargo', 'test', '-q', '-p', pkg, '--lib', name]
    # A hang counts as caught (README, "Planted bugs").
    r = subprocess.run(['timeout', '-k', '10', '600', *args], cwd=ROOT, env=ENV, capture_output=True, text=True)
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
            print(f'{i:2} NOT FOUND  {name}', flush=True)
            continue
        open(p, 'w').write(s.replace(old, new))
        try:
            code, out, broke = run(t)
        finally:
            open(p, 'wb').write(orig)
        if broke:
            print(f'{i:2} DID NOT COMPILE  {name}', flush=True)
        elif code != 0:
            caught += 1
            fails = [l.split()[1] for l in out.splitlines() if l.startswith('---- ')]
            if code == 124:
                fails = ['(timed out)', *fails]
            print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]', flush=True)
        else:
            print(f'{i:2} MISSED  {name}', flush=True)
    print(f'{caught} of {len(pick)} caught')


if __name__ == '__main__':
    main()
