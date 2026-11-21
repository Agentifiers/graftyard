#!/usr/bin/env python3
"""commit-hold: planted bugs in the 1.5 s commit hold and the edge's 2.5 s receipt (`decisions/DECISIONS.md`,
"`commit-hold`"). Plants one mutant at a time, runs the test that guards it, and restores the file byte for byte. Each
line prints CAUGHT, MISSED, or DID NOT COMPILE. Mutant 0 is main's hold. Run from the repository root:

    python3 tests/m0carried/plant80.py          # every mutant
    python3 tests/m0carried/plant80.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
RAFT = 'crates/raft/src/lib.rs'
LOOKUP = 'crates/lookup/src/lib.rs'
MEMBER = 'workers/site/src/member.rs'
EDGE = ['cargo', 'test', '-q', '-p', 'graftyard-lookup', '--lib', '--', 'a_commit_held_its_whole_hold']
SITE = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--', 'a_commit_names_its_hold_only_on_a_lean_send']

MUTANTS = [
    ("main's hold: 1 s", RAFT,
     'pub const COMMIT_HOLD: Duration = Duration::from_millis(1_500);',
     'pub const COMMIT_HOLD: Duration = Duration::from_secs(1);', EDGE),
    ("the receipt left at 2 s", LOOKUP,
     'pub const COMMIT_RECEIPT: Duration = Duration::from_millis(\n'
     '    graftyard_raft::COMMIT_HOLD.as_millis() as u64 + graftyard_raft::CALL_DEADLINE.as_millis() as u64,\n);',
     'pub const COMMIT_RECEIPT: Duration = Duration::from_millis(2_000);', EDGE),
    ("the receipt is the hold alone, with no call after it", LOOKUP,
     '    graftyard_raft::COMMIT_HOLD.as_millis() as u64 + graftyard_raft::CALL_DEADLINE.as_millis() as u64,\n',
     '    graftyard_raft::COMMIT_HOLD.as_millis() as u64,\n', EDGE),
    ("an `unknown` by request id ends the send", LOOKUP,
     '                if v.get("done").is_some() || v.pointer("/error/notLeader").is_some() {',
     '                if v.get("done").is_some() || v.pointer("/error/notLeader").is_some() || v.get("unknown").is_some() {',
     EDGE),
    ("a send stays open past RECEIPT_OPEN", LOOKUP,
     'pub const RECEIPT_OPEN: Duration = Duration::from_millis(8_000);',
     'pub const RECEIPT_OPEN: Duration = Duration::from_millis(12_000);', EDGE),
    ("the leader's product hold stays 1 s", MEMBER,
     '    hold_ms.map_or(raft::COMMIT_HOLD, Duration::from_millis)',
     '    hold_ms.map_or(Duration::from_secs(1), Duration::from_millis)', SITE),
]


def main():
    # A stop ends in SystemExit, so `finally` restores the file (plant75's lesson, `sync-ab.md`, "The plant left in").
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, test = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        text = orig.decode()
        if text.count(old) != 1:
            print(f'{i} {name}: anchor found {text.count(old)} times')
            sys.exit(2)
        try:
            open(p, 'w').write(text.replace(old, new))
            r = subprocess.run(test, cwd=ROOT, capture_output=True, text=True, timeout=1800)
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
