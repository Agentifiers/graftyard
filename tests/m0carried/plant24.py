#!/usr/bin/env python3
"""receipt-rule: planted bugs in the edge's commit receipt, its outcome lookup body, validate's record, and the shard's
`touch` (`decisions/DECISIONS.md`, "`receipt-rule`"). Plants one mutant at a time, runs the test that guards it, and
restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant24.py          # every mutant
    python3 tests/m0carried/plant24.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LOOKUP = 'crates/lookup/src/lib.rs'
EDGE = 'workers/edge/src/lib.rs'
SHARD = 'crates/git/src/shard.rs'
LOOK = ['cargo', 'test', '-q', '-p', 'graftyard-lookup', '--lib', '--', 'a_slow', 'a_member_that']
EDGET = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', '--', 'a_push_asks', 'a_validate_url']
TOUCH = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'shard', 'a_shard_records']

MUTANTS = [
    ('a missed receipt is never reported', LOOKUP,
     '        if !busy() {\n            return Receipt::Missed { report: true };\n        }\n',
     '        if false {\n            return Receipt::Missed { report: true };\n        }\n', LOOK),
    ('every missed receipt is reported, as on main', LOOKUP,
     '        if !busy() {\n            return Receipt::Missed { report: true };\n        }\n',
     '        if true {\n            return Receipt::Missed { report: true };\n        }\n', LOOK),
    ('`unknown` by request id is taken as the reply', LOOKUP,
     '                if v.get("done").is_some() || v.pointer("/error/notLeader").is_some() {',
     '                if v.get("done").is_some() || v.get("unknown").is_some() || v.pointer("/error/notLeader").is_some() {',
     LOOK),
    ('any error by request id is taken as the reply', LOOKUP,
     'v.pointer("/error/notLeader").is_some() {', 'v.get("error").is_some() {', LOOK),
    ('a pending ticket is passed on as an answer, not a ticket', LOOKUP,
     '                    return Receipt::Reply(json!({ "admitted": p, "meta": v.get("meta") }));',
     '                    return Receipt::Reply(v);', LOOK),
    ('an error status from a busy member is not reported', LOOKUP,
     '        Some(_) => Receipt::Missed { report: true },', '        Some(_) => Receipt::Missed { report: !busy },',
     LOOK),
    ('busy ignores the term', LOOKUP,
     'is_some_and(|(t, at)| *t >= term && now', 'is_some_and(|(_t, at)| now', LOOK),
    ('busy ignores the window', LOOKUP,
     '&& now.saturating_sub(*at) <= within))', '&& now.saturating_sub(*at) <= within * 100))', LOOK),
    ('busy ignores the group term this isolate saw', LOOKUP,
     '    let term = term.max(KNOWN.with(|k| k.borrow().get(&chain).map_or(0, |e| e.term)));\n', '', LOOK),
    ('an answer with no term counts as heard', LOOKUP,
     '    let Some(term) = v.pointer("/meta/term").and_then(Value::as_u64) else { return };',
     '    let term = v.pointer("/meta/term").and_then(Value::as_u64).unwrap_or(u64::MAX);', LOOK),
    ('the outcome lookup carries the pack', EDGE,
     '    let cmd = Push { pack: None, ..push.clone() };', '    let cmd = push.clone();', EDGET),
    ('validate carries no record', EDGE,
     '"https://shard/validate?repo={}&record={record}&tipsn={tipsn}"', '"https://shard/validate?repo={}&tipsn={tipsn}"',
     EDGET),
    ('every request writes last_request', SHARD,
     '    last_request.is_none_or(|l| now.0.saturating_sub(l.0) >= TOUCH_EVERY.as_millis() as u64)', '    true', TOUCH),
    ('a minute on, the request still does not write', SHARD,
     'now.0.saturating_sub(l.0) >= TOUCH_EVERY', 'now.0.saturating_sub(l.0) > TOUCH_EVERY', TOUCH),
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
