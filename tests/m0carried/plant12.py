#!/usr/bin/env python3
"""admission-stored-bytes: planted bugs in admission's pricing by `Command::stored_len` (`decisions/DECISIONS.md`,
"`admission-stored-bytes`"). Plants one mutant at a time, runs the raft and git tests in release, and restores the file
byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the
repository root:

    python3 tests/m0carried/plant12.py          # every mutant
    python3 tests/m0carried/plant12.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LEADER = 'crates/raft/src/member/leader.rs'
REPO = 'crates/git/src/repo.rs'
RAFT = ['cargo', 'test', '-q', '--release', '-p', 'graftyard-raft', '-p', 'graftyard-git']

MUTANTS = [
    ('admission prices the encoded command', LEADER,
     'let soft_bytes = cmd.stored_len().saturating_sub(agent_inline);', 'let soft_bytes = len.saturating_sub(agent_inline);'),
    ('the cut ignores ENTRY_SOFT', LEADER, '(b.0 + q.soft_bytes <= soft && b.1 + q.bytes <= ENTRY_MAX)',
     '(b.1 + q.bytes <= ENTRY_MAX)'),
    ('an issue command is priced with its render', REPO, '                self.encoded_len() - render.saturating_sub(4)\n',
     '                self.encoded_len() + 0 * render\n'),
    ('every command is priced without its pack too', REPO, '            _ => self.encoded_len(),\n        }\n    }\n}',
     '            _ => 1,\n        }\n    }\n}'),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i:2} NOT FOUND  {name}')
            continue
        open(p, 'w').write(s.replace(old, new))
        try:
            r = subprocess.run(RAFT, cwd=ROOT, capture_output=True, text=True, timeout=1800)
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
