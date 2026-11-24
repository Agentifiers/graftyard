#!/usr/bin/env python3
"""agent-rows-cache's policy rule: planted bugs in `decide`'s own protected-path check (`crates/git/src/repo.rs`,
`Push::agent_rules`; `crates/git/src/agent.rs`, `protected_now`; `decisions/DECISIONS.md`, "`agent-rows-cache`"). A push
whose `protected` the edge computed on an older `AgentPolicy` is refused when the fold's current `extra_paths` protect
one of its `changed` paths. Plants one mutant at a time, runs the test, and restores the file byte for byte. Each line
prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant151.py          # every mutant
    python3 tests/m0carried/plant151.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
T = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'agent_gates', '--',
     'a_policy_committed_after_the_edges_read_binds_the_next_push']
R, A = 'crates/git/src/repo.rs', 'crates/git/src/agent.rs'

MUTANTS = [
    ('decide trusts the edge\'s protected alone (main)', R,
     '                && let Some(p) = crate::agent::protected_now(c, &extra)\n',
     '                && let Some(p) = crate::agent::protected_now(c, &extra).filter(|_| false)\n'),
    ('the policy is read from the fold before this entry, not after its earlier commands', R,
     'let extra = crate::tiers::agent_policy(s.core()).extra_paths;',
     'let extra = crate::tiers::agent_policy(&*s.fold().core).extra_paths;'),
    ('only the policy\'s first glob is checked', A,
     'extra.iter().find_map(|g| c.paths.iter().find(|p| hit(g, p))).cloned()',
     'extra.iter().take(1).find_map(|g| c.paths.iter().find(|p| hit(g, p))).cloned()'),
    ('only the first changed path is checked', A,
     'extra.iter().find_map(|g| c.paths.iter().find(|p| hit(g, p))).cloned()',
     'extra.iter().find_map(|g| c.paths.iter().take(1).find(|p| hit(g, p))).cloned()'),
    ('a truncated directory is matched as a file path', A,
     "        if p.ends_with('/') { graftyard_identity::glob::glob_overlap(g, p) } else { glob_match(g, p) }\n",
     '        glob_match(g, p)\n'),
    ('every path is matched by overlap, so a prefix glob refuses its sibling file', A,
     "        if p.ends_with('/') { graftyard_identity::glob::glob_overlap(g, p) } else { glob_match(g, p) }\n",
     "        graftyard_identity::glob::glob_overlap(g, &format!(\"{p}/\"))\n"),
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
