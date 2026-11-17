#!/usr/bin/env python3
"""read-beat: planted bugs in a read round's empty AppendEntries to a voter with pipelined calls out
(`decisions/DECISIONS.md`, "`read-beat`"). Plants one mutant at a time, runs the raft crate's tests, and
restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant34.py          # every mutant
    python3 tests/m0carried/plant34.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LEADER = 'crates/raft/src/member/leader.rs'
TESTS = ['cargo', 'test', '-q', '-p', 'graftyard-raft']

MUTANTS = [
    ('the round still re-sends from next: no empty round', LEADER,
     '            if !self.read_beat(p) {\n', '            if true {\n', TESTS),
    ('the empty round sits on the last index, not the acknowledged one', LEADER,
     '        let matched = lead.matched(peer);\n',
     '        let matched = self.log.last().map_or(base, |e| e.index).max(lead.matched(peer));\n', TESTS),
    ('the empty round goes to a voter with nothing out', LEADER,
     "lead.pipe.get(&peer).is_none_or(Vec::is_empty) || !fresh || matched < base",
     "matched < base", TESTS),
    ('a call outstanding past READ_RESEND still gets only an empty round', LEADER,
     "lead.sent.get(&peer).is_some_and(|(_, at)| now < after(*at, READ_RESEND))",
     "lead.sent.contains_key(&peer)", TESTS),
    ('the empty round resets the pipeline, as a forced send did', LEADER,
     '        lead.round += 1;\n        let req = AppendReq {\n            term,\n            leader: me,\n            boot,\n            round: lead.round,',
     '        lead.round += 1;\n        lead.pipe.remove(&peer);\n        let req = AppendReq {\n            term,\n            leader: me,\n            boot,\n            round: lead.round,', TESTS),
    ('the empty round carries an old send time, so its ack grants nothing new', LEADER,
     '            round: lead.round,\n            sent_at: now,\n            prev_index: matched,',
     '            round: lead.round,\n            sent_at: Timestamp(now.0.saturating_sub(1_000)),\n            prev_index: matched,', TESTS),
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
