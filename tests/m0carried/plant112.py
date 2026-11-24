#!/usr/bin/env python3
"""agent-spill (G40): planted bugs in where an agent validate goes when a shard's gate is full
(`workers/edge/src/lib.rs`, `hedged` and `validate_failed`; `decisions/DECISIONS.md`, "`agent-spill`"): main's rule
(a 429 from any try ends the push, so a full home keeps every agent validate), a refusal that drops the agent's shape
when the sites it moved on to failed too (503 instead of `agent admission full`), the longest `retry-after` instead
of the shortest, and a 429 that starts every site left at once (a hedge for agents). Plants one mutant at a time,
runs the tests that guard it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT
COMPILE. Run from the repository root:

    python3 tests/m0carried/plant112.py          # every mutant
    python3 tests/m0carried/plant112.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
ED = 'workers/edge/src/lib.rs'
ED_T = (['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', '--', 'a_human_validate_asks', 'only_a_shards_422'],
        '.')

MUTANTS = [
    ("main's rule: a 429 from any try decides, so the push ends on the home's full gate", ED,
     "            Some((i, r @ (Ok(_) | Err((422, _))))) => {\n",
     "            Some((i, r @ (Ok(_) | Err((422 | 429, _))))) => {\n", ED_T),
    ("tries that end on full gates and failures are Unavailable (503), not the agent's shape", ED,
     "    if let Some(s) = tries.iter().filter(|(_, s, _)| *s == 429)",
     "    if let Some(s) = tries.iter().filter(|(_, s, _)| *s == 999)", ED_T),
    ("the refusal carries the longest retry-after a gate gave", ED,
     ".map(|(_, _, e)| e.parse().unwrap_or(1)).min() {\n",
     ".map(|(_, _, e)| e.parse().unwrap_or(1)).max() {\n", ED_T),
    ("a 429 starts every site left at once, as a hedge would", ED,
     "                due = usize::from(!out.hedged && out.started < n);\n",
     "                due = if out.failed.last().is_some_and(|(_, (s, _))| *s == 429) { n - out.started } else { usize::from(!out.hedged && out.started < n) };\n",
     ED_T),
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
