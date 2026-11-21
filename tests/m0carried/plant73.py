#!/usr/bin/env python3
"""retire-unjoined: planted bugs in an unjoined member's retire (`decisions/DECISIONS.md`, "`retire-sticks`",
Finding, re-retire). Plants one mutant at a time, runs the test that guards it, and restores the file byte for byte.
Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root; the bed's mutants need tests/m0's
wrangler (WRANGLER= to point at another checkout's):

    python3 tests/m0carried/plant73.py          # every mutant
    python3 tests/m0carried/plant73.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
RT = 'workers/site/src/retire.rs'
UNIT = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib',
        'an_unjoined_member_takes_a_retire_and_refuses_every_other_route']
BED = ['bash', 'tests/retirebed/run.sh']

MUTANTS = [
    # Main's rule: an unjoined member answers `/retire` 409 and keeps no row.
    ('an unjoined member refuses the test retire', RT,
     '        (Method::Post, "/retire") if test_faults => Unjoined::Retire { purge: false },\n', '', BED),
    ('an unjoined member refuses the purge retire', RT,
     '        (Method::Post, "/purge_retire") => Unjoined::Retire { purge: true },\n', '', BED),
    ('the test retire is served on a product build', RT,
     '        (Method::Post, "/retire") if test_faults =>', '        (Method::Post, "/retire") =>', UNIT),
    ('any POST on an unjoined member retires it', RT,
     '        _ => Unjoined::Refuse,\n', '        (Method::Post, _) => Unjoined::Retire { purge: true },\n        _ => Unjoined::Refuse,\n',
     BED),
    ('a retire of an unjoined member keeps no row', RT,
     '    if !reuse {\n', '    if reuse && !reuse {\n', BED),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    env = dict(os.environ)
    env.setdefault('WRANGLER', os.path.join(ROOT, 'tests/m0/node_modules/.bin/wrangler'))
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
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=900, env=env)
            out = r.stdout + r.stderr
        finally:
            open(p, 'wb').write(orig)
        if r.returncode == 0:
            verdict = 'MISSED'
        elif 'error[E' in out or 'could not compile' in out:
            verdict = 'DID NOT COMPILE'
        else:
            verdict = 'CAUGHT'
            caught += 1
        print(f'{i} {verdict}: {name}', flush=True)
    print(f'{caught} of {len(pick)} caught')
    sys.exit(0 if caught == len(pick) else 1)


if __name__ == '__main__':
    main()
