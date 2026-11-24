#!/usr/bin/env python3
"""`slot-bound` (G40's slow-home half, B0-33 h1A): planted bugs in how long an agent validate waits for a git cache
container's slot (`containers/git/server/main.go`, `validateGate.acquire`, `agentWait`; `workers/edge/src/lib.rs`,
`agent_slot_ms`; `decisions/DECISIONS.md`, "`slot-bound`"): main's whole 2 s wait however little of the push's bound is
left, a slot wait past the container's 2 s cap, a slot wait that keeps no reserve for the validate and its commit, a
queue that would not drain still held to its deadline, a drain estimate that counts the agent itself, and a slot_ms
the shard drops before the container. Plants one mutant at a time, runs the tests that guard it, and restores the file
byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant160.py          # every mutant
    python3 tests/m0carried/plant160.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
ED = 'workers/edge/src/lib.rs'
GO_MAIN = 'containers/git/server/main.go'
SHARD = 'workers/site/src/shard.rs'
ED_T = (['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', '--', 'an_agent_try_waits_for_a_slot'], '.')
GO_T = (['go', 'test', '-count=1', '-run', 'TestGate', '.'], 'containers/git/server')
# The shard's forward of slot_ms has no native test (the site's GitShard runs only in workerd); its mutant is checked
# by grep for the key the container reads.
SH_T = (['bash', '-c', 'grep -q \'"slot_ms"\' workers/site/src/shard.rs'], '.')

MUTANTS = [
    ("main's rule: an agent waits the container's whole 2 s whatever its bound leaves", ED,
     "    left.map(|l| l.saturating_sub(AGENT_SLOT_RESERVE).as_millis().min(2_000) as u64)\n",
     "    left.map(|_| 2_000)\n", ED_T),
    ("the slot wait is not capped at the container's 2 s", ED,
     "    left.map(|l| l.saturating_sub(AGENT_SLOT_RESERVE).as_millis().min(2_000) as u64)\n",
     "    left.map(|l| l.saturating_sub(AGENT_SLOT_RESERVE).as_millis() as u64)\n", ED_T),
    ("no reserve: the slot wait spends the whole bound left", ED,
     "    left.map(|l| l.saturating_sub(AGENT_SLOT_RESERVE).as_millis().min(2_000) as u64)\n",
     "    left.map(|l| l.as_millis().min(2_000) as u64)\n", ED_T),
    ("the container ignores slot_ms (main's 2 s)", GO_MAIN,
     "\treturn min(agentValidateWait, time.Duration(ms)*time.Millisecond)\n",
     "\treturn agentValidateWait\n", GO_T),
    ("a queue that will not drain in the wait is still held to its deadline", GO_MAIN,
     "\tif agent && (len(g.agents) >= agentQueuePerSlot*g.slots || g.drainLocked(len(g.agents)) > wait) {\n",
     "\tif agent && len(g.agents) >= agentQueuePerSlot*g.slots {\n", GO_T),
    ("the drain counts far more than the agents ahead (refuses agents that would be served)", GO_MAIN,
     "\treturn time.Duration(float64(ahead) / float64(g.slots) * g.average * float64(time.Second))\n",
     "\treturn time.Duration(float64(ahead+2) / float64(g.slots) * g.average * float64(time.Second))\n", GO_T),
    ("the shard drops slot_ms before the container", SHARD,
     '["headsum", "packsum", "slot_ms"]', '["headsum", "packsum"]', SH_T),
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
