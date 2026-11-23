#!/usr/bin/env python3
"""BF-47's local half: planted bugs in `release roll`'s vault step (`crates/release/src/ops.rs` `vault_due`,
`cli/src/release.rs` `vault_step` and `roll_in`; `decisions/DECISIONS.md`, `vault-roll`). Plants one mutant at a time,
runs the tests that guard it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT
COMPILE. Run from the repository root:

    python3 tests/m0carried/plant93.py          # every mutant
    python3 tests/m0carried/plant93.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
OPS = 'crates/release/src/ops.rs'
CLI = 'cli/src/release.rs'
GUARD = ['sh', '-c', 'cargo test -q -p graftyard-release -- bf47 release_roll && cargo test -q -p graftyard -- release_roll']

MUTANTS = [
    ("the vault step is due before the code intents are done", OPS,
     '.is_some_and(|up| up.files.iter().any(|(n, _)| n.starts_with(VAULT_FOLDER)) && next_code(fold, up, r).is_none())',
     '.is_some_and(|up| up.files.iter().any(|(n, _)| n.starts_with(VAULT_FOLDER)))', GUARD),
    ("a release with no vault Worker owes the vault step", OPS,
     '.is_some_and(|up| up.files.iter().any(|(n, _)| n.starts_with(VAULT_FOLDER)) && next_code(fold, up, r).is_none())',
     '.is_some_and(|up| next_code(fold, up, r).is_none())', GUARD),
    ("the vault step waits for the ContainerDays", OPS,
     '.is_some_and(|up| up.files.iter().any(|(n, _)| n.starts_with(VAULT_FOLDER)) && next_code(fold, up, r).is_none())',
     '.is_some_and(|up| up.files.iter().any(|(n, _)| n.starts_with(VAULT_FOLDER)) && next_roll(fold, r) == Roll::Done)',
     GUARD),
    ("release roll never takes the vault step", CLI,
     '    if vault_due(&r.fold, rid) {\n        println!("{}", vault_step(&t, rid, vault, creds)?);\n    }\n',
     '    let _ = (vault, creds, vault_due as fn(_, _) -> bool, vault_step as fn(_, _, _, _) -> _);\n', GUARD),
    ("the vault step takes the install's token", CLI,
     '    if creds.install == Some(token) {', '    if false && creds.install == Some(token) {', GUARD),
    ("the vault step takes the install's account", CLI,
     '    if v.account == t.account {', '    if false && v.account == t.account {', GUARD),
    ("the vault step upgrades a vault already at R", CLI,
     '    if crate::vault::serving(&cf, v.script)?.as_deref() == Some(rid.as_str()) {',
     '    if false && crate::vault::serving(&cf, v.script)?.as_deref() == Some(rid.as_str()) {', GUARD),
    ("the vault step takes another release's folder", CLI,
     '    if sha2::Sha256::digest(&record).as_slice() != r.0 {', '    if false && sha2::Sha256::digest(&record).as_slice() != r.0 {',
     GUARD),
    ("with no vault token the roll stops instead of owing the step", CLI,
     '        return Ok(owed(&format!("{} is not set", crate::vault::TOKEN_VAR)));',
     '        return Err(format!("{} is not set", crate::vault::TOKEN_VAR));', GUARD),
]


def main():
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
