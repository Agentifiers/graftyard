#!/usr/bin/env python3
"""prsync-first-once: planted bugs in a `PrSync` attempt's deadline and the test-merge line (`graftyard_git::prsync`:
`test_merges`, `merges_until`, `asked`'s TURN_WAIT_MS shed, and `BudgetStats::late`). Plants one mutant at a time,
runs the tests that gate it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE.
The site's wiring (`workers/site/src/prsync.rs`: `attempt` ending `one` at its deadline, and the leader waiting
FIRST_FOR plus ANSWER_MARGIN_MS) runs only in a Worker; its constants are checked at compile time and the rest is
covered hosted. Run from the worktree root:

    python3 tests/firstonce/plant.py          # every mutant
    python3 tests/firstonce/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
PS = 'crates/git/src/prsync.rs'
SYNC = ['cargo', '-p', 'graftyard-git', '--test', 'prsync']
GIT = ['cargo', '-p', 'graftyard-git', '--test', 'agent_load']

MUTANTS = [
    ('main: a batch asks every merge however late (no deadline)', PS,
     '        if now >= until {\n            out.unasked += 1;', '        if false && now >= until {\n            out.unasked += 1;',
     SYNC),
    ('main: a merge waits its turn however long (no TURN_WAIT_MS shed)', PS,
     '    if now.saturating_sub(queued) > TURN_WAIT_MS {', '    if false && now.saturating_sub(queued) > TURN_WAIT_MS {',
     SYNC + GIT),
    ('a late merge is counted shed but still runs', PS,
     '        s.late += 1;\n        return Ok(None);\n    }', '        s.late += 1;\n    }', SYNC),
    ('late sheds are not counted', PS, '        s.late += 1;\n', '', SYNC),
    ('merges ask until the attempt\'s very end (no commit reserve)', PS,
     '    end - COMMIT_RESERVE_MS.min(end.saturating_sub(start) / 2)', '    end', SYNC),
    ('a short deadline keeps the full reserve and asks nothing', PS,
     '    end - COMMIT_RESERVE_MS.min(end.saturating_sub(start) / 2)', '    end.saturating_sub(COMMIT_RESERVE_MS)', SYNC),
    ('a merge left unasked at the deadline is dropped, not shed', PS,
     '            out.unasked += 1;\n            out.shed.push((v.n, head));', '            out.unasked += 1;', SYNC),
    ('the line\'s wait counts from after the turn, so nothing is ever late', PS,
     '    let now = clock();\n    // Its turn came too late', '    let now = queued;\n    // Its turn came too late', SYNC),
]


ENV = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin') + ':' + os.environ['PATH'], CARGO_INCREMENTAL='0')


def run(t):
    out, broke = '', False
    # Each cargo target in the list, in turn; the first failure decides.
    for i in range(0, len(t), 5):
        _, pkg_flag, pkg, kind, name = t[i:i + 5]
        args = ['cargo', 'test', '-q', pkg_flag, pkg, kind, name]
        r = subprocess.run(args, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=1200)
        out += r.stdout + r.stderr
        broke = broke or 'error[E' in out or 'could not compile' in out
        if r.returncode != 0:
            return r.returncode, out, broke
    return 0, out, broke


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
        sys.stdout.flush()
    print(f'{caught} of {len(list(pick))} caught')


if __name__ == '__main__':
    main()
