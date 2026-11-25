#!/usr/bin/env python3
"""M5-WP6: planted bugs in `graft-ops`'s `Rotate` executor (`crates/release/src/group.rs` `rotate_steps` and the
switch in `apply`; `crates/release/src/ops.rs` `rotate_next` and `run`; `workers/ops/src/lib.rs` `rotate_command`;
`decisions/DECISIONS.md`, "`rotate-probe`"). Plants one mutant at a time, runs the tests that guard it, and restores the
file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Never run it under `timeout`: a kill leaves
the mutant in place. Run from the repository root:

    python3 tests/m0carried/plant95.py          # every mutant
    python3 tests/m0carried/plant95.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
GROUP = 'crates/release/src/group.rs'
OPS = 'crates/release/src/ops.rs'
LIB = 'workers/ops/src/lib.rs'
REL = ['cargo', 'test', '-q', '-p', 'graftyard-release', '--lib', '--', 'bf46', 'a_killed_graft_ops_reruns_a_rotate']
OPSLIB = ['cargo', 'test', '-q', '-p', 'graftyard-ops', '--lib', '--', 'a_mint_wrap_key_rotate']

MUTANTS = [
    ("a token switches with no probe", GROUP,
     'SecretName::DeployToken | SecretName::ReadToken => Ok(&[Probe, Switch]),',
     'SecretName::DeployToken | SecretName::ReadToken => Ok(&[Switch]),', REL),
    ("MintWrapKey switches before its key is published", GROUP,
     'SecretName::MintWrapKey => Ok(&[Publish, Wait, Switch, Retire]),',
     'SecretName::MintWrapKey => Ok(&[Switch, Publish, Wait, Retire]),', REL),
    ("MintWrapKey never retires the old key", GROUP,
     'SecretName::MintWrapKey => Ok(&[Publish, Wait, Switch, Retire]),',
     'SecretName::MintWrapKey => Ok(&[Publish, Wait, Switch]),', REL),
    ("HostedRunnersAppKey is rotated by graft-ops though no view holds its slot", GROUP,
     '        SecretName::HostedRunnersAppKey => {\n            Err(',
     '        SecretName::HostedRunnersAppKey => {\n            return Ok(&[Probe, Switch]);\n            #[allow(unreachable_code)]\n            Err(', REL),
    ("the switch step moves no slot", GROUP,
     '                    self.slots.insert(*secret, (*slot, at));', '                    let _ = (secret, slot);', REL),
    ("every step moves the slot, the probe included", GROUP,
     '&& rotate_steps(*secret).ok().and_then(|s| s.get(usize::from(c.next))) == Some(&RotateStep::Switch)',
     '&& rotate_steps(*secret).is_ok()', REL),
    ("the publish wait is skipped", OPS,
     '            if w.now < until {\n                Action::Wait { until: Some(until), why: format!("{secret:?}\'s new slot is announced").into() }',
     '            if false && w.now < until {\n                Action::Wait { until: Some(until), why: format!("{secret:?}\'s new slot is announced").into() }', REL),
    ("a refused Rotate is skipped to IntentDone instead of paging", OPS,
     '        Err(why) => return Action::Page(format!("intent {}: graft-ops refuses Rotate {secret:?}: {why}", id.0).into()),',
     '        Err(_) => return done,', REL),
    ("a Rotate to the active slot runs", OPS,
     '    if c.next == 0 && active_slot(fold, secret, w) == slot {',
     '    if false && c.next == 0 && active_slot(fold, secret, w) == slot {', REL),
    ("SecretsWrapKey's first Rotate reads the fold's default 0, not SECRETS_WRAP_KEY_CURRENT", OPS,
     '        (SecretName::SecretsWrapKey, None) => w.wrap_var,', '        (SecretName::SecretsWrapKey, None) => 0,', REL),
    ("a failed host step still commits its IntentStep", OPS,
     '                if let Err(x) = e.rotate(&fold, secret, slot, step).await {',
     '                if let Err(x) = e.rotate(&fold, secret, slot, step).await && false {', REL),
    ("an Abort of a Rotate runs its steps", OPS,
     '    if !matches!(c.state, IntentState::Running { .. }) {\n        return done;\n    }\n    // Refused',
     '    // Refused', REL),
    ("the old key retires at the switch, not after the longest token", LIB,
     "            TokensOp::PutVerifyingKey { kid, key: old.public(), retired: Some(at) }",
     "            let _ = at;\n            TokensOp::PutVerifyingKey { kid, key: old.public(), retired: Some(switched) }", OPSLIB),
    ("publish names the old key", LIB,
     "graftyard_release::RotateStep::Publish => TokensOp::PutVerifyingKey { kid, key: new.public(), retired: None },",
     "graftyard_release::RotateStep::Publish => {\n            let (kid, old) = unwrap(old_text)?;\n            let _ = new;\n            TokensOp::PutVerifyingKey { kid, key: old.public(), retired: None }\n        }", OPSLIB),
    ("an old slot with the new kid is retired", LIB,
     '            if old_kid == kid {', '            if false && old_kid == kid {', OPSLIB),
    ("a test install's publish wait is ignored (BF-46 hosted's OPS_ROTATE_PUBLISHED_MS)", OPS,
     '                _ => dwell.rotate_published,\n',
     '                _ => ROTATE_PUBLISHED_FOR + 0 * dwell.rotate_published,\n', REL),
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
