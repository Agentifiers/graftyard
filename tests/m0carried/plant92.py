#!/usr/bin/env python3
"""edge-refusal: planted bugs in the leader's measured agent drain, its advert, and the edge's refusal
(`decisions/DECISIONS.md`, "`edge-refusal`"). Plants one mutant at a time, runs the tests that guard it, and restores
the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Mutant 0 puts main's behaviour back (the
bucket's rate), so the new admission test fails on main. Run from the repository root:

    python3 tests/m0carried/plant92.py          # every mutant
    python3 tests/m0carried/plant92.py 0 3      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LEADER = 'crates/raft/src/member/leader.rs'
ADMIT = 'crates/raft/src/admit.rs'
LOOKUP = 'crates/lookup/src/lib.rs'
EDGE = 'workers/edge/src/lib.rs'
API = 'workers/edge/src/api.rs'
SLOW = ['cargo', 'test', '-q', '-p', 'graftyard-raft', '--test', 'admission', 'a_slow_leader']
DRAIN = ['cargo', 'test', '-q', '-p', 'graftyard-raft', '--lib', 'the_drain']
ADVERT = ['cargo', 'test', '-q', '-p', 'graftyard-lookup', '--lib', 'a_leader_advert']
EDGE_T = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', 'tests::']
RATE = '        self.drain.rate(now).map_or(bucket, |r| r.min(bucket)).max(1.0)\n'

MUTANTS = [
    ("main's rule: the bucket's rate", LEADER, RATE, '        bucket.max(1.0)\n', SLOW),
    ("the drain is not capped at the bucket", LEADER, RATE, '        self.drain.rate(now).unwrap_or(bucket).max(1.0)\n',
     ['cargo', 'test', '-q', '-p', 'graftyard-raft', '--test', 'admission']),
    ("the first cut after an idle gap counts", ADMIT, '        if let Some((at, true)) = self.last {\n',
     '        if let Some((at, _)) = self.last {\n', DRAIN),
    ("a fresh advert at the budget refuses", EDGE,
     "    let past = |a: &graftyard_lookup::Advert| u128::from(a.wait_ms) > budget.as_millis();\n",
     "    let past = |a: &graftyard_lookup::Advert| u128::from(a.wait_ms) >= budget.as_millis();\n", EDGE_T),
    ("a lapsed advert still refuses", EDGE, "        Heard::Lapsed(a, _) if past(&a) => AdvertSays::FailOpen,\n",
     "        Heard::Lapsed(a, _) if past(&a) => AdvertSays::Refuse(a),\n", EDGE_T),
    ("every class is refused", EDGE, "        (s, Some(graftyard_git::group::ClassWire::Agent)) if reaches => s,\n",
     "        (s, Some(_)) if reaches => s,\n", EDGE_T),
    ("a token that cannot reach the repository is refused", EDGE,
     "        (s, Some(graftyard_git::group::ClassWire::Agent)) if reaches => s,\n",
     "        (s, Some(graftyard_git::group::ClassWire::Agent)) => s,\n", EDGE_T),
    ("an installation token's repository list is not read", EDGE,
     "        Credential::Stateless(claims) => listed(&claims.repos),\n",
     "        Credential::Stateless(_) => true,\n", EDGE_T),
    ("the advert never lapses", LOOKUP, "        Some((a, at)) if now.saturating_sub(at) <= ttl => Heard::Fresh(a),\n",
     "        Some((a, _)) => Heard::Fresh(a),\n", ADVERT),
    ("a deposed leader's advert is kept", LOOKUP, "    if term < known_term(chain) {\n", "    if false && term < known_term(chain) {\n",
     ADVERT),
    ("a leader full on an issue command is 503 again", API,
     '    if let Some(s) = done.pointer("/full/retry_after_s").and_then(Value::as_u64) {\n',
     '    if let Some(s) = done.pointer("/full/never").and_then(Value::as_u64) {\n',
     ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', 'a_leader_full']),
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
