#!/usr/bin/env python3
"""m0-carried-7: planted bugs in the entry byte cap (`ENTRY_SOFT`, `tests/m0/results/carried-7.md`). Plants one mutant
at a time, runs the raft tests that gate it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or
DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant.py          # every mutant
    python3 tests/m0carried/plant.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LEADER = 'crates/raft/src/member/leader.rs'
LIB = 'crates/raft/src/lib.rs'
SCEN = ['cargo', 'test', '-q', '--release', '-p', 'graftyard-raft', '--test', 'scenarios']
ADMIT = ['cargo', 'test', '-q', '--release', '-p', 'graftyard-raft', '--test', 'admission']
RAFT = ['cargo', 'test', '-q', '--release', '-p', 'graftyard-raft']

CUT = 'batch.is_empty() || (b.0 + q.soft_bytes <= soft && b.1 + q.bytes <= ENTRY_MAX)'
MUTANTS = [
    ('the cut caps at ENTRY_MAX again (no soft cap)', LEADER, CUT,
     'batch.is_empty() || (b.1 + q.bytes <= ENTRY_MAX)', SCEN),
    ('the cut splits early, at half the cap', LEADER, CUT,
     'batch.is_empty() || (b.0 + q.soft_bytes <= soft / 2 && b.1 + q.bytes <= ENTRY_MAX)', SCEN),
    ('the first command no longer always goes, so a command over the cap never commits', LEADER, CUT,
     '(b.0 + q.soft_bytes <= soft && b.1 + q.bytes <= ENTRY_MAX)', RAFT),
    ('the cap off by one command (strict less-than plus a command)', LEADER, CUT,
     'batch.is_empty() || (b.0 <= soft && b.1 + q.bytes <= ENTRY_MAX)', SCEN),
    ('the bytes counter is not advanced for Human commands', LEADER,
     '                (bytes, taken[class.lane()]) = (add(bytes, &q), taken[class.lane()] + 1);',
     '                taken[class.lane()] += 1;', SCEN),
    # Since admission-stored-bytes (e96eeecb) the soft bytes are what the entry stores, less the inline pack.
    ('the soft cap counts Agent inline packs again (LM-1 round 9)', LEADER,
     'let soft_bytes = cmd.stored_len().saturating_sub(agent_inline);', 'let soft_bytes = cmd.stored_len();', ADMIT),
    ('capacity still sized from ENTRY_MAX, so admission over-admits', LEADER,
     'let per_entry = (soft as f32 / self.cmd_bytes.max(CMD_BYTES_MIN)).max(1.0);',
     'let per_entry = ENTRY_MAX as f32 / self.cmd_bytes.max(CMD_BYTES_MIN);', ADMIT),
    ('ENTRY_SOFT raised to ENTRY_MAX', LIB, 'pub const ENTRY_SOFT: usize = 100_000;',
     'pub const ENTRY_SOFT: usize = 1_500_000;', SCEN),
    ('a member starts uncapped (the default is ENTRY_MAX)', 'crates/raft/src/member/mod.rs',
     'entry_soft: crate::ENTRY_SOFT,', 'entry_soft: crate::ENTRY_MAX,', SCEN),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, args = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i:2} NOT FOUND  {name}')
            continue
        open(p, 'w').write(s.replace(old, new))
        try:
            r = subprocess.run(args, cwd=ROOT, capture_output=True, text=True, timeout=1800)
        finally:
            open(p, 'wb').write(orig)
        out = r.stdout + r.stderr
        if 'error[E' in out or 'could not compile' in out:
            print(f'{i:2} DID NOT COMPILE  {name}')
        elif r.returncode != 0:
            caught += 1
            fails = [l.split()[1] for l in out.splitlines() if l.startswith('---- ')]
            print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]')
        else:
            print(f'{i:2} MISSED  {name}')
        sys.stdout.flush()
    print(f'{caught} of {len(list(pick))} caught')


if __name__ == '__main__':
    main()
