#!/usr/bin/env python3
"""user-demote: planted bugs in DEMOTE_PER_MIN for plain user tokens (`crates/git/src/group.rs`, `AdmitWire::of` and
`for_issue`; `crates/raft/src/member/leader.rs`, `demoted`, `admit` and `admission_check`; `workers/site/src/host.rs`,
`Stats::admitted`). B0-33 h1A's fast user, 100 writes a minute on a plain PAT, was never demoted because the edge sent
no admission for a Human write. The mutants: the edge drops a user's Human admission again, the limit moves by one
either way, a task command counts, an App's class changes, an anonymous write sends an admission, and the leader or the
site misses a demotion or counts one that did not happen. Plants one mutant at a time, runs the tests that guard it,
and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant190.py          # every mutant
    python3 tests/m0carried/plant190.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
GR = 'crates/git/src/group.rs'
LE = 'crates/raft/src/member/leader.rs'
HO = 'workers/site/src/host.rs'
GR_T = (['cargo', 'test', '-q', '-p', 'graftyard-git', '--lib', '--', 'group::tests'], '.')
AG_T = (['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'agent_gates', '--', 'an_actor_past'], '.')
HO_T = (['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--', 'the_demotion_count'], '.')

MUTANTS = [
    ("a user token's Human write carries no admission (dc880f1d's behaviour)", GR,
     "            AdmitClass::Human => ClassWire::Human,\n",
     "            AdmitClass::Human => return None,\n", AG_T),
    ("a user at 59 writes in the minute is demoted", LE,
     "        if committed + held + refused >= DEMOTE_PER_MIN { AdmitClass::Agent } else { a.class }\n",
     "        if committed + held + refused >= DEMOTE_PER_MIN - 1 { AdmitClass::Agent } else { a.class }\n", AG_T),
    ("a user at 60 writes in the minute is not demoted", LE,
     "        if committed + held + refused >= DEMOTE_PER_MIN { AdmitClass::Agent } else { a.class }\n",
     "        if committed + held + refused > DEMOTE_PER_MIN { AdmitClass::Agent } else { a.class }\n", AG_T),
    ("task API commands count toward DEMOTE_PER_MIN", GR,
     "        admit.filter(|a| !(op.is_task() && a.class == ClassWire::Human))\n",
     "        admit\n", GR_T),
    ("an App bot token's write changes class", GR,
     "            AdmitClass::Bot => ClassWire::Bot,\n",
     "            AdmitClass::Bot => ClassWire::Human,\n", GR_T),
    ("an anonymous Human write sends an admission", GR,
     "            AdmitClass::Human if actor.is_none() => return None,\n",
     "", GR_T),
    ("the leader never reports a demotion", LE,
     "                    demoted = a.actor;\n",
     "", AG_T),
    ("the leader reports every actor as demoted", LE,
     "                if class != a.class {\n                    demoted = a.actor;\n",
     "                if true {\n                    demoted = a.actor;\n", AG_T),
    ("the site's counter misses a demotion", HO,
     "            *self.demoted.entry(actor.0).or_insert(0) += 1;\n",
     "            self.demoted.entry(actor.0).or_insert(0);\n", HO_T),
    ("reset keeps the demotion count", HO,
     "        self.demoted.clear();\n",
     "", HO_T),
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
