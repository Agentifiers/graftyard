#!/usr/bin/env python3
"""m0-adaptive-append: planted bugs in the late-answer pipelining gate (`pipe_lossy`), the per-link call bound
(`Lead::bound`), the pipeline's gap rule, and the fill of a pipeline to a voter behind, `tests/m0/results/carried-9.md`.
Plants one mutant at a time, runs the raft tests in release with the r300-3 scenario over `R300_SEEDS` (default 20..32),
and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant9.py          # every mutant
    python3 tests/m0carried/plant9.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LEADER = 'crates/raft/src/member/leader.rs'
HEALTH = 'crates/raft/src/member/health.rs'
RAFT = ['cargo', 'test', '-q', '--release', '-p', 'graftyard-raft']

MUTANTS = [
    ('pipelining stops on late answers (link_lossy)', LEADER,
     'let lossy = self.pipe_lossy(peer);', 'let lossy = self.link_lossy(peer);'),
    ('pipelining never stops', LEADER, 'let lossy = self.pipe_lossy(peer);', 'let lossy = false;'),
    ('a late answer still counts as lost after LATE_LIMIT', HEALTH,
     '            h.late.remove(&(to, round));\n', '            let _ = round;\n'),
    ('an unanswered miss never counts as lost', HEALTH,
     'h.pipe_calls.push_back((now, k.0, false));', 'let _ = k;'),
    ('a miss counts as lost at once, not after LATE_LIMIT', HEALTH,
     'after(**s, CALL_DEADLINE + LATE_LIMIT) <= now', 'after(**s, CALL_DEADLINE) <= now'),
    ('the bound never halves', LEADER, '            *b = (b.0 / 2, now);', '            *b = (b.0, now);'),
    ('the bound never grows back', LEADER,
     'b.0 = (b.0 + BOUND_STEP).min(APPEND_BYTES_MAX);', 'b.0 = b.0.min(APPEND_BYTES_MAX);'),
    ('the bound halves on every miss, answered late or not', HEALTH,
     '                    self.record_call(f.to, round, false, f.deadline, sent_at);\n',
     '                    self.record_call(f.to, round, false, f.deadline, sent_at);\n'
     '                    if f.deadline {\n                        self.bound_lost(f.to, sent_at);\n                    }\n'),
    ('a call over the bound carries no entry', LEADER, '        if n > 0 && bytes > max {', '        if bytes > max {'),
    ('a reordered call clears the pipe', LEADER, '            if gap && i > 0 {', '            if false && gap && i > 0 {'),
    ('a reordered call is resent at once', LEADER,
     '                lead.sent.insert(from, (p[i - 1].0, now));\n                return;\n',
     '                lead.sent.insert(from, (p[i - 1].0, now));\n                self.send_append(from, false);\n                return;\n'),
    ('a voter behind gets one call per answer', LEADER,
     '        if self.send_one(peer, force, false) {\n            while self.send_one(peer, false, false) {}\n        }',
     '        self.send_one(peer, force, false);'),
    ('a learner is not pipelined for a read', LEADER, 'let voter = read || quorum::is_voter(', 'let voter = quorum::is_voter('),
    ('a refusal of a call no longer in the pipe keeps it', LEADER,
     '        } else if !res.ok {\n            // A refusal of a call no longer in the pipe',
     '        } else if false {\n            // A refusal of a call no longer in the pipe'),
]


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    env = dict(os.environ, R300_SEEDS=os.environ.get('R300_SEEDS', '20..32'))
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
            r = subprocess.run(RAFT, cwd=ROOT, env=env, capture_output=True, text=True, timeout=1800)
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
