#!/usr/bin/env python3
"""F113's removed-voter answer: planted bugs in how a replaced voter learns that the group removed it and answers a
read-index read (`workers/site/src/member.rs`, `superseded_by` and `removed_answer`; `decisions/DECISIONS.md`,
"`removed-read`"). Plants one mutant at a time, runs the test that guards it, and restores the file byte for byte.
Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant106.py          # every mutant
    python3 tests/m0carried/plant106.py 0 3      # by number

`read`'s call to `removed_answer` and `check_superseded`'s registry and `/status` reads are not planted: they need a
member's bindings, which only the wasm build has. `tests/f113bed/run.sh` drives them in local workerd.
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
M = 'workers/site/src/member.rs'
T = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib',
     'member::tests::a_replaced_voter_answers_a_read_index_read_at_once_with_the_config_that_removed_it']
GEN = "    if row.chain != chain {\n"
BOTH = "    (removed(Some(&cfg), &me.do_name) && !removed(Some(&cfg), &row.do_name)).then_some((index, cfg))\n"
IDX = "    let s = superseded.filter(|_| index)?;\n"
CFG = "    let nl = raft::NotLeader { config: Some(s.clone()), ..nl.clone() };\n"

MUTANTS = [
    ("no change: a replaced voter never learns it was removed", M, BOTH, "    drop((index, cfg));\n    None\n", T),
    ("a try no config names yet counts as a replacement", M, BOTH,
     "    (!removed(Some(&cfg), &row.do_name)).then_some((index, cfg))\n", T),
    ("a config that drops the row's member too counts", M, BOTH,
     "    removed(Some(&cfg), &me.do_name).then_some((index, cfg))\n", T),
    ("another chain's row counts", M, GEN, "    if false {\n", T),
    ("a plain read on a removed member is refused too", M, IDX, "    let s = superseded?;\n", T),
    ("the answer carries the removed member's own stale config", M, CFG, "    let nl = nl.clone();\n", T),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
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
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=1800)
            out = r.stdout + r.stderr
            if r.returncode == 0:
                verdict = 'MISSED'
            elif 'error[E' in out or 'could not compile' in out:
                verdict = 'DID NOT COMPILE'
            else:
                verdict = 'CAUGHT'
                caught += 1
        finally:
            open(p, 'wb').write(orig)
        print(f'{i} {name}: {verdict}', flush=True)
    print(f'caught {caught} of {len(pick)}')
    sys.exit(0 if caught == len(pick) else 1)


if __name__ == '__main__':
    main()
