#!/usr/bin/env python3
"""admitted-bytes: planted bugs in what a leader and its host hold for open tickets (`decisions/DECISIONS.md`,
"`admitted-bytes`"). Plants one mutant at a time, runs the tests that guard it in release, and restores the file byte
for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant14.py          # every mutant
    python3 tests/m0carried/plant14.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LEADER = 'crates/raft/src/member/leader.rs'
REPO = 'crates/git/src/repo.rs'
RAFT = ['cargo', 'test', '-q', '--release', '-p', 'graftyard-raft', '--test', 'scenarios', '--test', 'deps']
GIT = ['cargo', 'test', '-q', '--release', '-p', 'graftyard-git', '--test', 'admitted_memory']

MUTANTS = [
    ('a deposed leadership answers under the new term', LEADER,
     'let tickets: Vec<Ticket> = seqs.map(|seq| Ticket { term, boot, seq }).collect();',
     'let tickets: Vec<Ticket> = seqs.map(|seq| self.ticket(seq)).collect();\n        let _ = (term, boot);', RAFT),
    ('the same, seen by the host', LEADER,
     'let tickets: Vec<Ticket> = seqs.map(|seq| Ticket { term, boot, seq }).collect();',
     'let tickets: Vec<Ticket> = seqs.map(|seq| self.ticket(seq)).collect();\n        let _ = (term, boot);', GIT),
    ('a resumed term restarts its seqs', LEADER,
     'seq: self.lead.as_ref().map_or(self.seqs, |l| l.seq).max(self.seqs),', 'seq: 0,', RAFT),
    ('the host keeps the whole command', REPO,
     '        match self {\n            RepoCmd::Push(p) if matches!(p.pack, Some(PackRef::Stored { .. })) =>',
     '        if self.encoded_len() > 0 {\n            return Some(self.clone());\n        }\n        match self {\n            RepoCmd::Push(p) if matches!(p.pack, Some(PackRef::Stored { .. })) =>', GIT),
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
        open(p, 'w').write(s.replace(old, new))
        try:
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=1800)
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
