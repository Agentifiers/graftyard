#!/usr/bin/env python3
"""promote-warm: planted bugs in this branch's code (promote-ahead's `/promote` as a warm call that starts no stopped
shard, and the `warm` query read as a warm caller on every route). Plants one mutant at a time, runs the tests that
gate it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the
worktree root:

    python3 tests/promotewarm/plant.py          # every mutant
    python3 tests/promotewarm/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LA = 'crates/git/src/log_archive.rs'
SH = 'crates/git/src/shard.rs'
ARCHIVE = ['-p', 'graftyard-git', '--test', 'log_archive']
SHARD = ['-p', 'graftyard-git', '--test', 'shard']

MUTANTS = [
    ('promote-ahead asks as a client, as on main', LA, '    let warm = d.is_none();\n', '    let warm = false;\n',
     ARCHIVE),
    ('every promotion is warm, so a due range never starts a shard', LA, '    let warm = d.is_none();\n',
     '    let warm = true;\n', ARCHIVE),
    # `promote` answers None when every shard is stopped and the call was warm: the step is idle, not failed.
    ('a skipped promotion is an error, not idle', LA,
     '                f.at = now;\n            }\n            return Ok(Step::Idle);',
     '                f.at = now;\n            }\n            return Err("skipped".into());', ARCHIVE),
    ('warm counts only on /catchup, as on main', SH, '        Some(_) => Caller::Warm,',
     '        Some(_) if path == "/catchup" => Caller::Warm,\n        Some(_) => Caller::Client,', SHARD),
    ('pool warm on every route', SH, '        Some("pool") if path == "/catchup" => Caller::PoolWarm,',
     '        Some("pool") => Caller::PoolWarm,', SHARD),
    ('a call without warm is a warm call', SH, '        None => Caller::Client,', '        None => Caller::Warm,', SHARD),
]

ENV = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin') + ':' + os.environ['PATH'], CARGO_INCREMENTAL='0',
           CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, 'target', 'plants')))


def run(t):
    # A hang counts as caught (README, "Planted bugs").
    r = subprocess.run(['timeout', '-k', '10', '600', 'cargo', 'test', '-q'] + t, cwd=ROOT, env=ENV, capture_output=True,
                       text=True)
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
            if code == 124:
                fails = ['(timed out)', *fails]
            print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]')
        else:
            print(f'{i:2} MISSED  {name}')
    print(f'{caught} of {len(pick)} caught')


if __name__ == '__main__':
    main()
