#!/usr/bin/env python3
"""stale-log and the F91 switch: planted bugs in the stale check and the edge's ls-refs switch
(`decisions/DECISIONS.md`, "`stale-log`"). Plants one mutant at a time, runs the test that guards it, and restores the
file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant45.py          # every mutant
    python3 tests/m0carried/plant45.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
CHECK = 'tests/fanout/stalecheck.mjs'
EDGE = 'workers/edge/src/lib.rs'
CHECKT = ['node', '--test', 'tests/fanout/stalecheck.test.mjs']
EDGET = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', '--', 'ls_refs_go']

MUTANTS = [
    ('an acked previous push passes the Stale', CHECK,
     '      out.push({ ...row, verdict: prev.new === p.old ? "FAIL" : "PASS",\n',
     '      out.push({ ...row, verdict: "PASS",\n', CHECKT),
    ('a lost reply with no edge line passes', CHECK,
     '    if (!e) { out.push({ ...row, verdict: "UNKNOWN",', '    if (!e) { out.push({ ...row, verdict: "PASS",', CHECKT),
    ('a lost reply passes whatever the leader answered', CHECK,
     '    const committed = (e) => e?.end?.refs?.length > 0 && e.end.refs.every((r) => r === "Ok");\n',
     '    const committed = (e) => !!e?.end;\n', CHECKT),
    ('overlapping pushes are not a race', CHECK,
     '    const overlap = same.some((q) => q !== p && q.start < p.end && p.start < q.end);\n',
     '    const overlap = false;\n', CHECKT),
    ('a refused previous push passes the Stale', CHECK,
     '      out.push({ ...row, verdict: tip === p.old ? "FAIL" : "PASS",\n',
     '      out.push({ ...row, verdict: "PASS",\n', CHECKT),
    ('a Stale is judged only against the push just before it', CHECK,
     '      const tip = acked && refusedSince ? acked.new : prev.old;\n',
     '      const tip = prev.old;\n', CHECKT),
    ('a lost reply since the ack is ignored', CHECK,
     '      const refusedSince = since.every((q) => q.status === 200 && q.result !== "ok");\n',
     '      const refusedSince = true;\n', CHECKT),
    ('the header alone turns the edge answer off', EDGE,
     '    !matches!((header, token, secret), (Some("shard"), Some(t), Some(s)) if t == s)\n',
     '    !matches!(header, Some("shard"))\n', EDGET),
    ('the switch is off by default', EDGE,
     '    !matches!((header, token, secret), (Some("shard"), Some(t), Some(s)) if t == s)\n',
     '    matches!((header, token, secret), (Some("edge"), Some(t), Some(s)) if t == s)\n', EDGET),
    ('any client id reaches the log line', EDGE,
     "    h.filter(|v| !v.is_empty() && v.len() <= 64 && v.bytes().all(|b| b.is_ascii_alphanumeric() || b\"._-\".contains(&b)))\n",
     '    h.filter(|v| !v.is_empty())\n', EDGET),
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
