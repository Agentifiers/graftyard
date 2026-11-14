#!/usr/bin/env python3
"""memory-budgets: planted bugs in the three memory budgets (`decisions/DECISIONS.md`, "`memory-budgets` (1)" to
"(3)"). Plants one mutant at a time, runs the tests that guard it in release, and restores the file byte for byte.
Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant13.py          # every mutant
    python3 tests/m0carried/plant13.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LEADER = 'crates/raft/src/member/leader.rs'
HEALTH = 'crates/raft/src/member/health.rs'
HOST = 'workers/site/src/host.rs'
MEMBER = 'workers/site/src/member.rs'
RAFT = ['cargo', 'test', '-q', '--release', '-p', 'graftyard-raft', '--test', 'scenarios']
GIT = ['cargo', 'test', '-q', '--release', '-p', 'graftyard-git', '--test', 'done_memory']
SITE = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--', 'member::']

MUTANTS = [
    # (1) `done` by bytes and age
    ('done keeps tickets past OUTCOME_KEEP', LEADER,
     '(lead.done.len() > DONE_KEEP || lead.done_bytes > DONE_BYTES || *at < old)',
     '(lead.done.len() > DONE_KEEP || lead.done_bytes > DONE_BYTES || *at < Timestamp(0))', RAFT),
    ('done ignores DONE_BYTES', LEADER,
     '(lead.done.len() > DONE_KEEP || lead.done_bytes > DONE_BYTES || *at < old)',
     '(lead.done.len() > DONE_KEEP || *at < old)', GIT),
    # (2) the outgoing byte budget
    ('every call is allowed', LEADER, '        if bytes == 0 {\n            return true;\n        }\n        let learner',
     '        if bytes < usize::MAX {\n            return true;\n        }\n        let learner', RAFT),
    ('the learners share no cap', LEADER, '(!learner(peer) || learners + bytes <= LEARNERS_OUT_MAX)', 'true', RAFT),
    ('a missed deadline ends the call', HEALTH, 'if boot == self.boot && !f.deadline {', 'if boot == self.boot {',
     RAFT),
    ('an answer does not end its call', LEADER,
     '        if res.boot == self.boot {\n            self.calls_out.remove(&(from, res.round));\n        }\n', '', RAFT),
    ('call_ended ends nothing', HEALTH,
     '        self.begin(now);\n        self.calls_out.remove(&(to, round));\n        self.finish(())',
     '        self.begin(now);\n        let _ = (to, round);\n        self.finish(())', RAFT),
    # (3) distinct answer bytes
    ('a shared answer counts per read', HOST, '        *n += 1;\n        if *n == 1 {\n            a.bytes += v.len();',
     '        *n += 1;\n        {\n            a.bytes += v.len();', SITE),
    ('the gate ignores answer bytes', MEMBER, '        || answers >= ANSWER_BYTES_MAX\n', '', SITE),
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
