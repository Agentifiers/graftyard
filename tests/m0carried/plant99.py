#!/usr/bin/env python3
"""edge-refusal-bound: planted bugs in the edge's hold on a lapsed full advert and its send check
(`decisions/DECISIONS.md`, "`edge-refusal-bound`"). Plants one mutant at a time, runs the tests that guard it, and
restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant99.py          # every mutant
    python3 tests/m0carried/plant99.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
EDGE = 'workers/edge/src/lib.rs'
EDGE_T = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', 'tests::']
AGENT_ARM = "        (s, Some(graftyard_git::group::ClassWire::Agent)) if reaches => s,\n"
PROBE = "    already || graftyard_lookup::take_probe(chain, now)\n"

MUTANTS = [
    ("a Human write is held on a lapsed advert (flip R1a)", EDGE, AGENT_ARM,
     "        (s @ AdvertSays::Hold(..), _) => s,\n" + AGENT_ARM, EDGE_T),
    ("no ADVERT_STALE: a lapsed full advert holds forever", EDGE,
     "        Heard::Lapsed(a, age) if past(&a) && age <= stale => AdvertSays::Hold(a, age),\n",
     "        Heard::Lapsed(a, age) if past(&a) || age <= stale => AdvertSays::Hold(a, age),\n", EDGE_T),
    ("no probe: every write on a lapsed advert is held", EDGE, PROBE, "    already\n", EDGE_T),
    ("a probe for every write: the hold holds nothing", EDGE, PROBE,
     "    already || { graftyard_lookup::take_probe(chain, now); true }\n", EDGE_T),
    ("the send check on the whole bound, not the budget left at the send", EDGE,
     "At::Send, std::time::Duration::from_millis(ms)).await\n",
     "At::Send, graftyard_raft::admit::AGENT_PUSH_BOUND).await\n", EDGE_T),
    ("no check before the Stage re-send", EDGE, "        if let Some(ms) = a.budget_ms\n",
     "        if let Some(ms) = None::<u64>\n", EDGE_T),
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
    print(f'{caught} of {len(pick)} caught')
    sys.exit(0 if caught == len(pick) else 1)


if __name__ == '__main__':
    main()
