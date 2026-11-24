#!/usr/bin/env python3
"""`moved-mark` (G41): planted bugs in when the edge marks a shard call as moved load (`workers/edge/src/lib.rs`,
`note`, `moved` and `shard_url`; `decisions/DECISIONS.md`, "`moved-mark`"): a hedge pass that counts as a failed home
(mt21's 573 s hold on site b), a pass that hides a live failure, a mark that never lapses, the mark read off the
called site rather than the home, the home's own call marked, and a URL that drops `moved=1`. Plants one mutant at a
time, runs the tests that guard it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT
COMPILE. Run from the repository root:

    python3 tests/m0carried/plant115.py          # every mutant
    python3 tests/m0carried/plant115.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
ED = 'workers/edge/src/lib.rs'
ED_T = (['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', '--', 'only_a_failed_home',
         'every_shard_try_but_the_last'], '.')

MUTANTS = [
    ("a hedge pass marks the home failed (mt21: site b held 573 s on a live home)", ED,
     "        Seen::Passed => m.get(&key).is_some_and(|c| c.until > now && c.failed),\n",
     "        Seen::Passed => true,\n", ED_T),
    ("a pass clears a live failure's mark", ED,
     "        Seen::Passed => m.get(&key).is_some_and(|c| c.until > now && c.failed),\n",
     "        Seen::Passed => false,\n", ED_T),
    ("a failed home stays moved past COLD_SHARD_MS", ED,
     "    site != home && m.get(&(net, home.0)).is_some_and(|c| c.until > now && c.failed)\n",
     "    site != home && m.get(&(net, home.0)).is_some_and(|c| c.failed)\n", ED_T),
    ("moved reads the called site's mark, not the home's", ED,
     "    site != home && m.get(&(net, home.0)).is_some_and(|c| c.until > now && c.failed)\n",
     "    site != home && m.get(&(net, site.0)).is_some_and(|c| c.until > now && c.failed)\n", ED_T),
    ("the home's own call is marked moved", ED,
     "    site != home && m.get(&(net, home.0)).is_some_and(|c| c.until > now && c.failed)\n",
     "    m.get(&(net, home.0)).is_some_and(|c| c.until > now && c.failed)\n", ED_T),
    ("the URL drops moved=1", ED,
     '    let moved = if moved { "&moved=1" } else { "" };\n',
     '    let moved = if false { "&moved=1" } else { "" };\n', ED_T),
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
