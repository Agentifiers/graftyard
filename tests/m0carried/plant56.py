#!/usr/bin/env python3
"""home-quiet and lookup-keep: planted bugs in the transfer back to `home` (`decisions/DECISIONS.md`, "`home-quiet`")
and in the lookup walk (`decisions/DECISIONS.md`, "`lookup-keep`"; B0-35 m58's term 4, F101). Plants one mutant at a
time, runs the tests that guard it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT
COMPILE. Run from the repository root:

    python3 tests/m0carried/plant56.py          # every mutant
    python3 tests/m0carried/plant56.py 0 5      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
HEALTH = 'crates/raft/src/member/health.rs'
LEADER = 'crates/raft/src/member/leader.rs'
LOOKUP = 'crates/lookup/src/lib.rs'
LEAD = ['cargo', 'test', '-q', '-p', 'graftyard-raft', '--test', 'leadership', '--', 'home', 'cap']
LOOK = ['cargo', 'test', '-q', '-p', 'graftyard-lookup', '--lib']

MUTANTS = [
    ("main's rule: the transfer home goes as soon as it is due", HEALTH,
     '        if self.quiet(HOME_QUIET_QUEUE) || now >= after(due, HOME_DEFER_MAX) {\n',
     '        if true {\n', LEAD),
    ("no cap: a busy leader never goes home", HEALTH,
     '        if self.quiet(HOME_QUIET_QUEUE) || now >= after(due, HOME_DEFER_MAX) {\n',
     '        if self.quiet(HOME_QUIET_QUEUE) {\n', LEAD),
    ("the cap restarts every tick", HEALTH,
     '.health.home_due.get_or_insert(now);\n',
     '.health.home_due.insert(now);\n', LEAD),
    ("quiet ignores the queue", LEADER,
     '        queued <= queue && entry <= self.entry_soft / 2\n',
     '        entry <= self.entry_soft / 2\n', LEAD),
    ("quiet ignores the entry in flight", LEADER,
     '        queued <= queue && entry <= self.entry_soft / 2\n',
     '        queued <= queue\n', LEAD),
    ("main's walk: a superseded bootstrap name is posted", LOOKUP,
     '            .filter(|m| !superseded(m))\n',
     '            .filter(|_| true)\n', LOOK),
    ("superseded ignores the generation: a move's new members are skipped", LOOKUP,
     '        known.generation >= m.generation && known.voters.iter()',
     '        known.voters.iter()', LOOK),
    ("main's pass end: the leader is always forgotten", LOOKUP,
     '        let kept = !kept_once\n',
     '        let kept = false && !kept_once\n', LOOK),
    ("one follower's word keeps a leader", LOOKUP,
     '.find(|(l, by)| by.len() >= 2 && misses.get(*l) == Some(&1))\n',
     '.find(|(l, by)| !by.is_empty() && misses.get(*l) == Some(&1))\n', LOOK),
    ("a kept leader is kept on every pass", LOOKUP,
     '        kept_once |= kept;\n',
     '        let _ = kept;\n', LOOK),
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
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=1800)
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
