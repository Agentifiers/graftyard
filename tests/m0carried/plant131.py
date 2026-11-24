#!/usr/bin/env python3
"""`source-record`: planted bugs in where a bootstrap join of a system chain reads its source's record
(`crates/lookup/src/lib.rs`, `source_record`; `workers/site/src/member.rs`, `check_source`; `tests/m6/results/m6-wp1.md`,
"Bootstrap refusals on cold isolates"). Each mutant needs a member's bindings, which only the wasm build has, so each
runs `tests/sourcebed/run.sh` on a fresh site build (about 4 minutes). Plants one mutant at a time and restores the
file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant131.py          # every mutant
    python3 tests/m0carried/plant131.py 0        # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LOOKUP = 'crates/lookup/src/lib.rs'
MEMBER = 'workers/site/src/member.rs'
T = ['bash', 'tests/sourcebed/run.sh']
CHECK = "                crate::release::source_record(&self.host.env, join.chain).await.map_err(|e| json!({ \"noRecord\": e }))?\n"
GEN = "        None => genesis(b, anchor).await?,\n"
CMP = "        if !source.lookup_order().any(|r| *r == join.record) {\n"

MUTANTS = [
    ("the bootstrap check reads the record through system_record's registry shortcut, as on 05ea366d", MEMBER, CHECK,
     CHECK.replace("source_record", "system_record"), T),
    ("a cold isolate's source record is built from a registry row", LOOKUP, GEN,
     "        None => match registry_record(b, chain).await {\n"
     "            Some(r) => return Ok(r),\n"
     "            None => genesis(b, anchor).await?,\n"
     "        },\n", T),
    ("the check compares only the generation, so a record that differs from the genesis joins", MEMBER, CMP,
     "        if !source.lookup_order().any(|r| r.generation == join.record.generation) {\n", T),
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
    print(f'caught {caught} of {len(pick)}')
    sys.exit(0 if caught == len(pick) else 1)


if __name__ == '__main__':
    main()
