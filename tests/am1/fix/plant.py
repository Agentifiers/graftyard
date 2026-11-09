#!/usr/bin/env python3
"""AM1 fix: planted bugs in the two fixes (the follower's stall marker in `crates/raft`, and the feed's shared owner
read in `workers/site`). Plants one mutant at a time, runs the tests that gate it, and restores the file byte for byte.
Each line prints CAUGHT with the failing tests, MISSED, or DID NOT COMPILE. Never run it while a site day builds from
this worktree. Run from the worktree root:

    python3 tests/am1/fix/plant.py          # every mutant
    python3 tests/am1/fix/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..', '..'))
FOL = 'crates/raft/src/member/follower.rs'
FEED = 'workers/site/src/feed.rs'
LEASE = ['-p', 'graftyard-git', '--test', 'lease_gates', 'l0_12']
SITE = ['-p', 'graftyard-site', '--lib', 'feed']
RAFT = ['-p', 'graftyard-raft', '--test', 'scenarios', 'stall']

MARK = ('        if let Some(at) = unacked.map(|e| e.at).min() {\n'
        '            self.state.stalled_since = Some(self.state.stalled_since.map_or(at, |m| m.min(at)));\n'
        '        }\n')

# (name, file, old, new, test args)
MUTANTS = [
    ('a follower never marks the stall from a re-send (the bug as hosted)', FOL, MARK, '', LEASE),
    ('a follower marks the re-send time, not the entry\'s at', FOL,
     '        if let Some(at) = unacked.map(|e| e.at).min() {',
     '        if let Some(at) = unacked.map(|_| req.sent_at).min() {', LEASE),
    ('a follower keeps its later marker over the entry\'s earlier at', FOL,
     '            self.state.stalled_since = Some(self.state.stalled_since.map_or(at, |m| m.min(at)));',
     '            self.state.stalled_since = Some(self.state.stalled_since.unwrap_or(at));', LEASE),
    ('a follower marks every re-sent entry, however fresh', FOL,
     '            .filter(|e| e.index <= matched && e.term == req.term && req.sent_at >= after(e.at, CALL_DEADLINE));',
     '            .filter(|e| e.index <= matched && e.term == req.term);', RAFT),
    ('a follower marks entries the group already committed', FOL,
     '            .take_while(|e| e.index > self.state.commit.max(req.commit))\n',
     '            .take_while(|_| true)\n', RAFT),
    ('every poll on a stale owner reads it (the herd as hosted)', FEED,
     '            Some(w) => {\n                let (tx, rx) = oneshot::channel();\n                w.push(tx);\n                (Refresh::Wait, Some(rx))\n            }',
     '            Some(_) => (Refresh::Read, None),', SITE),
    ('the waiting polls are never woken', FEED,
     '        for w in self.refreshing.take().unwrap_or_default() {\n            let _ = w.send(());\n        }',
     '        self.refreshing.take();', SITE),
    ('a finished read stays in flight', FEED,
     '    fn refreshed(&mut self) {\n        for w in self.refreshing.take().unwrap_or_default() {',
     '    fn refreshed(&mut self) {\n        for w in self.refreshing.as_mut().map(std::mem::take).unwrap_or_default() {', SITE),
    ('a fresh owner is read again', FEED,
     '        if !known || self.owner_at + 60_000 >= now {', '        if !known {', SITE),
]


def cargo(args):
    env = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin:') + os.environ['PATH'],
               CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, '..', 'target-am1')),
               CARGO_INCREMENTAL='0')
    *pkg, filt = args
    return subprocess.run(['cargo', 'test', '-j', '3', *pkg, '--no-fail-fast', filt], cwd=ROOT, env=env,
                          capture_output=True, text=True)


def plant(n, name, path, old, new, args):
    f = os.path.join(ROOT, path)
    orig = open(f, 'rb').read()
    src = orig.decode()
    assert src.count(old) == 1, (name, 'anchor not unique', src.count(old))
    open(f, 'w').write(src.replace(old, new, 1))
    try:
        r = cargo(args)
        out = r.stdout + r.stderr
        failed = sorted({l.split()[1] for l in out.splitlines() if l.startswith('test ') and l.endswith('FAILED')})
        if ('error[' in out or '\nerror: could not compile' in out) and not failed:
            verdict = 'DID NOT COMPILE'
        else:
            verdict = 'CAUGHT' if r.returncode != 0 and failed else 'MISSED'
        print(f'{n:2} {verdict:8} {name}: {", ".join(failed)}', flush=True)
    finally:
        open(f, 'wb').write(orig)
    assert open(f, 'rb').read() == orig


only = sys.argv[1:] and set(int(x) for x in sys.argv[1:])
for n, (name, path, old, new, args) in enumerate(MUTANTS):
    if not only or n in only:
        plant(n, name, path, old, new, args)
