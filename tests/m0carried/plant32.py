#!/usr/bin/env python3
"""read-ahead: planted bugs in the leader holding more than one frozen batch behind the entry in flight
(`decisions/DECISIONS.md`, "`read-ahead`"). Plants one mutant at a time, runs the test that guards it, and
restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant32.py          # every mutant
    python3 tests/m0carried/plant32.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LEADER = 'crates/raft/src/member/leader.rs'
DEPS = ['cargo', 'test', '-q', '-p', 'graftyard-raft', '--test', 'deps']

MUTANTS = [
    ('one frozen batch at a time, as on main', LEADER,
     '    ((deps_ms / round_ms.max(ROUND_MIN_MS)).ceil() as usize).clamp(1, READ_AHEAD)', '    1', DEPS),
    ('every later batch is due one round less a read after the entry in flight', LEADER,
     '        let wait = ((held + 1) as f32 * round_ms - deps_ms).max(0.0) as u64;',
     '        let wait = (round_ms - deps_ms).max(0.0) as u64;', DEPS),
    ('a read answers the batch cut last, whichever asked', LEADER,
     '            && let Some(d) = lead.deciding.iter_mut().find(|d| d.id == Some(id))',
     '            && let Some(d) = lead.deciding.back_mut()', DEPS),
    ('the newest batch is decided first', LEADER,
     '        let d = lead.deciding.front_mut().expect("a held batch");',
     '        let d = lead.deciding.back_mut().expect("a held batch");', DEPS),
    ('the re-read is asked for the batch cut last', LEADER,
     '            self.ask_deps_for(true, missing);', '            self.ask_deps_for(false, missing);', DEPS),
    ('a transfer drops only the front batch\'s reads', LEADER,
     '        for d in self.lead.as_mut().map(|l| l.deciding.iter_mut()).into_iter().flatten() {',
     '        for d in self.lead.as_mut().and_then(|l| l.deciding.front_mut()) {', DEPS),
    ('a held batch is decided while an entry is in flight', LEADER,
     '            if let Some((index, _)) = lead.pending {\n                self.cut_early(index);\n                return;\n            }',
     '            if let Some((index, _)) = lead.pending {\n                self.cut_early(index);\n                if self.lead.as_ref().is_some_and(|l| l.deciding.front().is_some_and(|d| d.id.is_none())) {\n                    self.decide_held();\n                }\n                return;\n            }',
     DEPS),
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
