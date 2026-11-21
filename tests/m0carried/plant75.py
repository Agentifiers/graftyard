#!/usr/bin/env python3
"""sync-ab: planted bugs in the leader-sync A/B's lean switch (`graftyard_lookup::Hooks::lean`, the site's
`CommitReq::hold_ms`, the edge's `lean_asked`; `tests/am1/results/sync-ab.md`). Plants one mutant at a time, runs the
tests that guard it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from
the repository root:

    python3 tests/m0carried/plant75.py          # every mutant
    python3 tests/m0carried/plant75.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LOOKUP = 'crates/lookup/src/lib.rs'
EDGE = 'workers/edge/src/lib.rs'
MEMBER = 'workers/site/src/member.rs'
LK = ['cargo', 'test', '-q', '-p', 'graftyard-lookup', '--lib', '--', 'a_lean_send_holds_its_commit',
      'a_slow_200_from_a_busy_member']
ED = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', '--', 'ls_refs_go_to_the_shard_only_when_a_bed_request_asks']
SI = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--', 'a_commit_names_its_hold_only_on_a_lean_send']

MUTANTS = [
    ("a lean send carries no hold", LOOKUP, 'let field = format!("{{\\"hold_ms\\":{LEAN_HOLD_MS}");',
     'let field = String::from("{");', LK),
    ("an empty body gets a trailing comma", LOOKUP,
     "let comma = if rest.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'}') { \"\" } else { \",\" };",
     'let comma = ",";', LK),
    ("a held answer outlives the send's deadline", LOOKUP, 'pub const LEAN_HOLD_MS: u64 = 6_000;',
     'pub const LEAN_HOLD_MS: u64 = 9_000;', LK),
    ("a lean send with no answer is always reported", LOOKUP,
     '        None => Receipt::Missed { report: !busy() },\n    }\n}\n\n/// A command',
     '        None => Receipt::Missed { report: true },\n    }\n}\n\n/// A command', LK),
    ("a lean send's error status is taken as its reply", LOOKUP,
     '        Some((200, v)) => Receipt::Reply(v),\n        Some(_) => Receipt::Missed { report: true },\n        None => Receipt::Missed { report: !busy() },',
     '        Some((_, v)) => Receipt::Reply(v),\n        None => Receipt::Missed { report: !busy() },', LK),
    ("the receipt asks only once however long the send stays open", LOOKUP,
     '            Either::Right(_) if !busy() => return Receipt::Missed { report: true },\n            Either::Right(_) => {}',
     '            Either::Right(_) if !busy() => return Receipt::Missed { report: true },\n            Either::Right(_) => return ended(send.await, busy()),', LK),
    ("the header switches lean without the bed token", EDGE,
     '    header && matches!((token, secret), (Some(t), Some(s)) if t == s)', '    header', ED),
    ("the site ignores the send's hold", MEMBER,
     '    hold_ms.map_or(raft::COMMIT_HOLD, Duration::from_millis)', '    let _ = hold_ms;\n    raft::COMMIT_HOLD', SI),
    ("a body with no hold is held longer than COMMIT_HOLD", MEMBER,
     '    hold_ms.map_or(raft::COMMIT_HOLD, Duration::from_millis)', '    hold_ms.map_or(Duration::from_secs(6), Duration::from_millis)',
     SI),
]


def main():
    # A stop (SIGTERM, as an orb service's stop sends) ends in SystemExit, so `finally` restores the file: a killed run
    # once left mutant 4 in the tree, and it was committed (`tests/am1/results/sync-ab.md`, "The plant left in").
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
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
