#!/usr/bin/env python3
"""R45: planted bugs in the step lists that `rotate-holders` gave the install-account secrets graft-ops used to refuse
(`crates/release/src/group.rs` `rotate_steps`; `crates/release/src/ops.rs` `rotate_next`, `SSH_ANNOUNCE_FOR`;
`decisions/DECISIONS.md`, "`rotate-holders`"). Plants one mutant at a time, runs the tests that guard it, and restores
the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Never run it under `timeout`: a kill
leaves the mutant in place. Run from the repository root:

    python3 tests/m0carried/plant213.py          # every mutant
    python3 tests/m0carried/plant213.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
GROUP = 'crates/release/src/group.rs'
OPS = 'crates/release/src/ops.rs'
REL = ['cargo', 'test', '-q', '-p', 'graftyard-release', '--lib', '--', 'r45_', 'bf46']

MUTANTS = [
    ("SshHostKeys switches with no announce wait", GROUP,
     '        SecretName::SshHostKeys => Ok(&[Probe, Wait, Switch]),',
     '        SecretName::SshHostKeys => Ok(&[Probe, Switch]),', REL),
    ("SshHostKeys switches before its probe", GROUP,
     '        SecretName::SshHostKeys => Ok(&[Probe, Wait, Switch]),',
     '        SecretName::SshHostKeys => Ok(&[Switch, Probe, Wait]),', REL),
    ("an App credential switches with no probe of the App's record", GROUP,
     '        SecretName::McpAppKey | SecretName::McpClientSecret | SecretName::InsightsAppKey => Ok(&[Probe, Switch]),',
     '        SecretName::McpAppKey | SecretName::McpClientSecret | SecretName::InsightsAppKey => Ok(&[Switch]),', REL),
    ("an App credential stays refused", GROUP,
     '        SecretName::McpAppKey | SecretName::McpClientSecret | SecretName::InsightsAppKey => Ok(&[Probe, Switch]),',
     '        SecretName::McpAppKey | SecretName::McpClientSecret | SecretName::InsightsAppKey => Err("refused"),', REL),
    ("SecretsWrapKey switches with no probe", GROUP,
     '        SecretName::SecretsWrapKey => Ok(&[Probe, Switch]),',
     '        SecretName::SecretsWrapKey => Ok(&[Switch]),', REL),
    ("the SSH announce waits MintWrapKey's publish time", OPS,
     '            let wait = if secret == SecretName::SshHostKeys { dwell.ssh_announce } else { dwell.rotate_published };',
     '            let wait = dwell.rotate_published;', REL),
    ("the SSH announce is a day, not 30", OPS,
     'pub const SSH_ANNOUNCE_FOR: u64 = 30 * 24 * 3_600_000;',
     'pub const SSH_ANNOUNCE_FOR: u64 = 24 * 3_600_000;', REL),
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
