#!/usr/bin/env python3
"""lm1-memory: planted bugs in this branch's new code (the lookup's overload answer and `suspect` damping, the member's
heavy-read cap, the packed-dump cache, and the edge's coalescing of tier reads). Plants one mutant at a time, runs the
tests that gate it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run
from the worktree root:

    python3 tests/lm1mem/plant.py          # every mutant
    python3 tests/lm1mem/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LK = 'crates/lookup/src/lib.rs'
MEM = 'workers/site/src/member.rs'
FAM = 'workers/site/src/family.rs'
CO = 'workers/edge/src/coalesce.rs'
LOOKUP = ['cargo', '-p', 'graftyard-lookup', '--lib', 'tests']
SITE = ['cargo', '-p', 'graftyard-site', '--lib', '']
EDGE = ['cargo', '-p', 'graftyard-edge', '--lib', 'coalesce']

MUTANTS = [
    ('lookup: an overloaded answer is taken as a missed member (suspect feeds the storm)', LK,
     '        return Answer::Overloaded(o.get("retryAfterMs").and_then(Value::as_u64).unwrap_or(250));',
     '        let _ = o;\n        return Answer::Missed("overloaded".into());', LOOKUP),
    ('lookup: an overloaded answer is returned to the caller as the read', LK,
     '    if let Some(o) = v.get("overloaded") {', '    if let Some(o) = v.get("overloaded").filter(|_| false) {', LOOKUP),
    ('lookup: a 5xx is no longer reported', LK,
     'if status >= 500 { Answer::Missed(status.to_string()) }', 'if status >= 600 { Answer::Missed(status.to_string()) }',
     LOOKUP),
    ('lookup: the overload reply drops `busy`, so an older lookup returns it as the read', LK,
     'json!({ "busy": true, "overloaded": { "retryAfterMs": retry_after_ms } })',
     'json!({ "overloaded": { "retryAfterMs": retry_after_ms } })', LOOKUP),
    ('damping: keyed without the term, so a new term\'s report is dropped', LK,
     '        match r.get(&(chain, about, term, all)) {', '        match r.get(&(chain, about, 0, all)) {', LOOKUP),
    ('damping: the gap never ends', LK, '            Some(at) if now < *at + gap => false,',
     '            Some(_) => false,', LOOKUP),
    ('cap: a follower sheds too', MEM, '    heavy && leading && in_flight >= HEAVY_READS',
     '    heavy && in_flight >= HEAVY_READS', SITE),
    ('cap: narrow row reads count as heavy', MEM, '    q.get("rows").is_some_and(Value::is_boolean)',
     '    q.get("rows").is_some()', SITE),
    ('cap: a dropped read stays counted', MEM, '        self.0.set(self.0.get().saturating_sub(1));',
     '        let _ = self.0;', SITE),
    ('cache: a hit is served without comparing its key, so an applied entry is not in the next read', FAM,
     '.filter(|(at, _)| at == k)', '.filter(|(at, _)| at == k || true)', SITE),
    ('cache: the key is a constant', FAM, '    (c.applied.0, c.sum, fold.meta.loaded(), m.applied.0, m.sum)',
     '    let _ = (c, m);\n    (0, [0; 32], false, 0, [0; 32])', SITE),
    ('coalesce: a caller joins the call already in flight', CO,
     '                if let Some(waiters) = round {\n                    waiters.push(tx);',
     '                if let Some(waiters) = round.as_mut().filter(|_| false) {\n                    waiters.push(tx);', EDGE),
    ('coalesce: a dropped leader hands off nothing', CO,
     '        if self.armed {\n            hand_off(&self.key);', '        if self.armed && false {\n            hand_off(&self.key);',
     EDGE),
    ('coalesce: the key stays in flight after the last round', CO,
     '        let Some(mut waiting) = rounds.remove(key).flatten() else { return };',
     '        let Some(mut waiting) = rounds.get_mut(key).and_then(Option::take) else { return };', EDGE),
]

ENV = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin') + ':' + os.environ['PATH'], CARGO_INCREMENTAL='0',
           CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', '/home/user/workspace/repo/.worktrees/target-rows'))


def run(t):
    if t[0] == 'node':
        r = subprocess.run(t, cwd=ROOT, capture_output=True, text=True, timeout=120)
        out = r.stdout + r.stderr
        return r.returncode, out, 'SyntaxError' in out
    if t[0] == 'go':
        r = subprocess.run(['go', 'test', '-count=1', '-run', t[1], '.'], cwd=os.path.join(ROOT, 'containers/git/server'),
                           capture_output=True, text=True, timeout=600)
        out = r.stdout + r.stderr
        return r.returncode, out, 'build failed' in out or 'syntax error' in out or 'undefined:' in out
    _, _, pkg, kind, name, *rest = t
    if kind == '--lib':
        args = ['cargo', 'test', '-q', '-p', pkg, '--lib', name]
    else:
        args = ['cargo', 'test', '-q', '-p', pkg, '--test', name] + [f for f in rest if f]
    r = subprocess.run(args, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=1200)
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
            fails = [l.split()[1] for l in out.splitlines() if l.startswith('---- ') or l.startswith('--- FAIL')]
            print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]')
        else:
            print(f'{i:2} MISSED  {name}')
        sys.stdout.flush()
    print(f'{caught} of {len(list(pick))} caught')


if __name__ == '__main__':
    main()
