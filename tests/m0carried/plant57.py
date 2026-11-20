#!/usr/bin/env python3
"""feed-per-member: planted bugs in the activity feed's per-member state and in the isolate-state check
(`decisions/DECISIONS.md`, "`feed-per-member`"; B0-29 m3fp3's shared isolate). Plants one mutant at a time, runs the
tests that guard it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run
from the repository root:

    python3 tests/m0carried/plant57.py          # every mutant
    python3 tests/m0carried/plant57.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
FEED = 'workers/site/src/feed.rs'
LIB = 'workers/site/src/lib.rs'
TESTS = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--', 'feed::tests', 'isolate_state']

MUTANTS = [
    ("main's key: every member of a repository shares one feed", FEED,
     '        match f.get(member) {\n            Some((b, feed)) if *b == boot => feed.clone(),\n',
     '        let member = member.split(\'#\').next().unwrap_or(member);\n'
     '        match f.get(member) {\n            Some((b, feed)) if *b == boot || true => feed.clone(),\n', TESTS),
    ("a new boot keeps the old boot's feed", FEED,
     '            Some((b, feed)) if *b == boot => feed.clone(),\n',
     '            Some((_, feed)) => feed.clone(),\n', TESTS),
    ("status reads another boot's feed", FEED,
     'f.borrow().get(member).filter(|(b, _)| *b == boot)',
     'f.borrow().get(member)', TESTS),
    ("the check lists FEEDS under main's repository key", LIB,
     '        ("FEEDS", "RefCell<BTreeMap<Box<str>, Kept>>"),\n',
     '        ("FEEDS", "RefCell<BTreeMap<RepoId"),\n', TESTS),
    ("the check skips structs that hold a sender", LIB,
     '                (ty.contains("Sender<") || holders.iter().any(named)).then_some("oneshot::Sender")\n',
     '                (ty.contains("Sender<") && holders.iter().any(named)).then_some("oneshot::Sender")\n', TESTS),
    ("a new isolate-wide sender list keyed by repository", FEED,
     '    static FEEDS: RefCell<BTreeMap<Box<str>, Kept>> = const { RefCell::new(BTreeMap::new()) };\n',
     '    static FEEDS: RefCell<BTreeMap<Box<str>, Kept>> = const { RefCell::new(BTreeMap::new()) };\n'
     '    static WAITING: RefCell<BTreeMap<RepoId, Vec<oneshot::Sender<bool>>>> = const { RefCell::new(BTreeMap::new()) };\n',
     TESTS),
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
