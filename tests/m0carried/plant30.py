#!/usr/bin/env python3
"""burst-receipts: planted bugs in the receipt's ask before report and the pool shard's send-back
(`decisions/DECISIONS.md`, "`burst-receipts`"). Plants one mutant at a time, runs the test that guards it, and
restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant30.py          # every mutant
    python3 tests/m0carried/plant30.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LOOKUP = 'crates/lookup/src/lib.rs'
SHARD = 'crates/git/src/shard.rs'
RECEIPT = ['cargo', 'test', '-q', '-p', 'graftyard-lookup', '--lib', 'reply_with_no_resend']
REPORTED = ['cargo', 'test', '-q', '-p', 'graftyard-lookup', '--lib', 'never_answered']
POOL = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'disk_validate', 'catch_up_back']

MUTANTS = [
    ('a member not busy is reported at the deadline, unasked, as on main', LOOKUP,
     '            return ended(r, busy());\n        }\n        match select(',
     '            return ended(r, busy());\n        }\n        if !busy() {\n            return Receipt::Missed { report: true };\n        }\n        match select(',
     RECEIPT),
    ('a member with no answer to the ask is never reported', LOOKUP,
     '            Either::Right(_) if !busy() => return Receipt::Missed { report: true },\n',
     '', REPORTED),
    ('an `unknown` to the ask ends the send as a reply', LOOKUP,
     '                if v.get("done").is_some() || v.pointer("/error/notLeader").is_some() {',
     '                if v.get("done").is_some() || v.pointer("/error/notLeader").is_some() || v.get("unknown").is_some() {',
     RECEIPT),
    ('an error status to the ask counts as an answer', LOOKUP,
     '            Either::Right(_) if !busy() => return Receipt::Missed { report: true },',
     '            Either::Right((Some(_), _)) => {}\n            Either::Right(_) if !busy() => return Receipt::Missed { report: true },',
     REPORTED),
    ('a pool shard waits for its catch-up, as on main', SHARD,
     '    if pool { OnCatchUp::Back } else { OnCatchUp::Wait }', '    OnCatchUp::Wait', POOL),
    ('the first shard sends a push back too', SHARD,
     '    if pool { OnCatchUp::Back } else { OnCatchUp::Wait }', '    OnCatchUp::Back', POOL),
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
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=900)
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
