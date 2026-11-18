#!/usr/bin/env python3
"""cut-trace: planted bugs in the one-clock queue split and the per-cut note (`decisions/DECISIONS.md`, "`cut-trace`").
Plants one mutant at a time, runs the tests that should catch it, and restores the file byte for byte. Each line prints
CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant39.py          # every mutant
    python3 tests/m0carried/plant39.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
HOST = 'workers/site/src/host.rs'
LEADER = 'crates/raft/src/member/leader.rs'
SITE = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', 'host::tests']
CUTS = ['cargo', 'test', '-q', '-p', 'graftyard-raft', '--test', 'admission', 'each_cut']

MUTANTS = [
    ("the queue on the member clock: main's rule", HOST,
     '        self.queue_ms.push(c.raw.saturating_sub(admitted) as u32);\n',
     '        self.queue_ms.push(c.member.saturating_sub(admitted) as u32);\n', SITE),
    ("the split's end on the member clock: main's rule", HOST,
     '        let (slot, post) = queue_split(admitted, self.last_apply_at, c.raw);\n',
     '        let (slot, post) = queue_split(admitted, self.last_apply_at, c.member);\n', SITE),
    ("the last apply on the member clock: main's rule", HOST,
     '        self.last_apply_at = c.raw;\n', '        self.last_apply_at = c.member;\n', SITE),
    ('a cut that ran out of tokens reads as one past its bytes', LEADER,
     '            (None, Some(_)) if ad.tokens < 1.0 => CutEnd::AgentTokens,\n', '', CUTS),
    ('ENTRY_SOFT and ENTRY_MAX swapped', LEADER,
     'if bytes.0 + q.soft_bytes > soft { CutEnd::Soft } else { CutEnd::EntryMax }',
     'if bytes.0 + q.soft_bytes > soft { CutEnd::EntryMax } else { CutEnd::Soft }', CUTS),
    ('a queued Human command ignored: the end read from the agent lane only', LEADER,
     '            (Some(q), _) => past(q),\n', '            (Some(_), None) => CutEnd::Empty,\n            (Some(q), Some(_)) => past(q),\n', CUTS),
    ('left reports what was taken, not what is still queued', LEADER,
     '        let left = AdmitClass::ALL.map(|c| ad.lanes[c.lane()].len() as u32);\n',
     '        let left = taken;\n', CUTS),
    ('a cut stops counting its Human commands', LEADER,
     '                (bytes, taken[class.lane()]) = (add(bytes, &q), taken[class.lane()] + 1);\n',
     '                bytes = add(bytes, &q);\n', CUTS),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, cmd = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i:2} NOT FOUND  {name}')
            continue
        # A copy beside the file, so a run killed mid-mutant can be undone with `git checkout` or this copy.
        open(p + '.orig', 'wb').write(orig)
        open(p, 'w').write(s.replace(old, new))
        try:
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=1800)
        finally:
            open(p, 'wb').write(orig)
            os.remove(p + '.orig')
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
