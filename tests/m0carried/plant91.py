#!/usr/bin/env python3
"""`reask-read`: planted bugs in the re-asked read of a frozen `Empty` batch (`crates/raft/src/member/leader.rs`,
`cut_early`, `deps_read`, `top_up` and `decide_held`; `decisions/DECISIONS.md`, "`reask-read`"). Plants one mutant at a
time, runs `crates/raft/tests/deps.rs`, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT
COMPILE. Mutant 0 is main's rule: no batch is re-asked. Never run it under `timeout`: a kill leaves the mutant in place.
Run from the repository root:

    python3 tests/m0carried/plant91.py          # every mutant
    python3 tests/m0carried/plant91.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
L = 'crates/raft/src/member/leader.rs'
T = ['cargo', 'test', '-q', '--release', '-p', 'graftyard-raft', '--test', 'deps']

MUTANTS = [
    ("main's rule: no batch is re-asked", L,
     '        d.open = note.end == CutEnd::Empty && note.in_flight', '        d.open = false && note.in_flight'),
    ("a backlogged cut is re-asked too (human-join's scope)", L,
     '        d.open = note.end == CutEnd::Empty && note.in_flight', '        d.open = note.in_flight'),
    ("the top-up takes commands admitted after the re-ask was asked", L,
     '            .filter(|q| q.0 <= upto)\n', '            .filter(|q| q.0 <= u64::MAX)\n'),
    ("the re-ask's folds go beside the batch's first read, not in place of it", L,
     '                d.deps = deps;\n', '                d.deps.extend(deps);\n'),
    ("DepsUsed still names the first read beside the re-ask", L,
     '                d.ids = SmallVec::from_slice(&[id]);\n', '                d.ids.push(id);\n'),
    ("a re-ask that lacks a chain still replaces the batch's reads", L,
     '            if chains.iter().all(|c| deps.iter().any(|(k, _)| k == c)) {',
     '            if chains.iter().all(|c| deps.iter().any(|(k, _)| k == c)) || true {'),
    ("the batch waits for an unanswered re-ask once the entry before it applied", L,
     '        d.open = false;\n        d.reask = None;\n        if d.id.is_some() {',
     '        d.open = false;\n        if d.id.is_some() || d.reask.is_some() {'),
    ("the top-up ignores the batch's soft bytes", L,
     '            if s + qs > soft || b + qb > ENTRY_MAX ||', '            if b + qb > ENTRY_MAX ||'),
    ("the top-up ignores AGENT_CMDS_ENTRY", L,
     'agent && (d.agents >= AGENT_CMDS_ENTRY as u32 || ad.tokens < 1.0)', 'agent && ad.tokens < 1.0'),
    ("a new re-ask leaves while one is out", L,
     '            if d.id.is_none() && d.reask.is_none() && lead.transfer.is_none()',
     '            if d.id.is_none() && lead.transfer.is_none()'),
]


def main():
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        text = orig.decode()
        if text.count(old) != 1:
            print(f'{i} {name}: anchor found {text.count(old)} times')
            sys.exit(2)
        try:
            open(p, 'w').write(text.replace(old, new))
            r = subprocess.run(T, cwd=ROOT, capture_output=True, text=True, errors='replace', timeout=1800,
                               stdin=subprocess.DEVNULL)
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
