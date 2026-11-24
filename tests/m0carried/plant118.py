#!/usr/bin/env python3
"""M6-WP1, a group left out of `ReplaceMember` (`crates/types/src/registry.rs`, `replace_candidates`;
`crates/identity/src/group_move.rs`, `holds` and `replace_step`): candidates at a lower generation than a surviving
row names, a mid-`Joint` group whose new half holds the lost site, and a `*2w` group whose lost Full member is moved
by `Joint` instead of `Degraded`, whose old half never commits.

Plants one mutant at a time, runs the tests that guard it, and restores the file byte for byte. Each line prints
CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant118.py          # every mutant
    python3 tests/m0carried/plant118.py 0 1      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
REG = 'crates/types/src/registry.rs'
MOVE = 'crates/identity/src/group_move.rs'
REG_T = (['cargo', 'test', '-q', '-p', 'graftyard-types', '--lib', '--', 'every_chain_on_a_surviving_site'], '.')
MOVE_T = (['cargo', 'test', '-q', '-p', 'graftyard-identity', '--test', 'group_move'], '.')

MUTANTS = [
    ("a chain's candidate is its lowest generation, not its highest", REG,
     "        if r.generation > e.generation {\n", "        if r.generation < e.generation {\n", REG_T),
    ("a Joint holds the lost site only in its old half", MOVE,
     "        GroupConfig::Joint { old, new } => on(old) || on(new),\n",
     "        GroupConfig::Joint { old, new } => on(old) || (on(new) && false),\n", MOVE_T),
    ("a *2w group's lost Full member goes by Joint, with no Degraded first", MOVE,
     "            if p.voters().iter().any(|m| m.site == lost && m.kind == graftyard_types::MemberKind::Full)\n",
     "            if p.voters().iter().any(|m| m.site == lost && m.kind == graftyard_types::MemberKind::Learner)\n",
     MOVE_T),
]

def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, (cmd, cwd) = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        text = orig.decode()
        if text.count(old) != 1:
            print(f'{i} {name}: anchor found {text.count(old)} times')
            sys.exit(2)
        try:
            open(p, 'w').write(text.replace(old, new))
            r = subprocess.run(cmd, cwd=os.path.join(ROOT, cwd), capture_output=True, text=True, timeout=1800)
            out = r.stdout + r.stderr
            if r.returncode == 0:
                verdict = 'MISSED'
            elif 'error[E' in out or 'could not compile' in out or '[build failed]' in out:
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
