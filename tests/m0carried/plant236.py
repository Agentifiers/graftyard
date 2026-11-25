#!/usr/bin/env python3
"""`actions-oidc` (DECISIONS "`actions-oidc`", `crates/actions/src/oidc.rs`): a token minted for a job another attempt
holds, for a fork's pull request, or for a job without the grant; `id-token` left on the job's `GITHUB_TOKEN`; the
token URL sent to every job; a `sub` that ignores the environment or the pull request; a `kid` that is not RFC 7638's;
a JWKS that drops a slot; a 1,024-bit key taken; `runner_environment` always `self-hosted`; `ActionsOidcKey` without
its wait or waiting another secret's; and `graft-ops`' probe passing a slot that holds the active key or differs by site.
Run from the repository root:

    python3 tests/m0carried/plant236.py          # every mutant
    python3 tests/m0carried/plant236.py 0 3      # by number

`$CARGO_TARGET_DIR` defaults to `target/plants`. Each run goes through `timeout -k 10 600`, and a run it ended counts as
caught (README, "Planted bugs").
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
O = 'crates/actions/src/oidc.rs'
TK = 'crates/actions/src/token.rs'
RS = 'crates/actions/src/runsvc.rs'
G = 'crates/release/src/group.rs'
OP = 'crates/release/src/ops.rs'
L = 'workers/ops/src/lib.rs'
ACT = ['-p', 'graftyard-actions', '--lib']
REL = ['-p', 'graftyard-release', '--lib']
OPS = ['-p', 'graftyard-ops', '--lib']
MUTANTS = [
    ('another attempt\'s token mints', O, '    if run.attempt != attempt {\n', '    if false {\n', ACT),
    ('a fork\'s pull request mints', O, '    if run.fork {\n', '    if false {\n', ACT),
    ('a job without the grant mints', O, '    !fork && declared.and_then(|p| p.0.get(ID_TOKEN_SCOPE)) == Some(&Access::Write)\n',
     '    !fork && declared.is_some()\n', ACT),
    ('write-all does not grant id-token', TK, '            m.insert(ID_TOKEN_SCOPE.into(), Access::Write);\n', '', ACT),
    ('the GITHUB_TOKEN carries id-token', TK, '    p.0.remove(crate::oidc::ID_TOKEN_SCOPE);\n', '', ACT),
    ('every job gets the token URL', RS, '    if m.id_token {\n', '    if true {\n', ACT),
    ('a pull_request_target run takes the ref sub', O,
     '        None if matches!(event, "pull_request" | "pull_request_target") => "pull_request".into(),\n',
     '        None if event == "pull_request" => "pull_request".into(),\n', ACT),
    ('a colon in the sub is left as is', O, '    let esc = |s: &str| s.replace(\':\', "%3A");\n',
     '    let esc = |s: &str| s.to_owned();\n', ACT),
    ('the kid hashes another member order', O,
     'r#"{{"e":"{}","kty":"RSA","n":"{}"}}"#, b64u(k.e()), b64u(k.n())',
     'r#"{{"kty":"RSA","e":"{}","n":"{}"}}"#, b64u(k.e()), b64u(k.n())', ACT),
    ('the JWKS lists one slot', O, '        .filter(|k| seen.insert(k["kid"].as_str().unwrap_or_default().to_owned()))\n',
     '        .filter(|k| seen.insert(k["kid"].as_str().unwrap_or_default().to_owned()))\n        .take(1)\n', ACT),
    ('a 1,024-bit key is taken', O, '    if k.n().bits() < KEY_BITS_MIN {\n', '    if k.n().bits() < 1024 {\n', ACT),
    ('a hosted runner says self-hosted', O, 'if f.hosted { "github-hosted" } else { "self-hosted" }',
     'if false { "github-hosted" } else { "self-hosted" }', ACT),
    ('a token lives ten minutes', O, '        "exp": now_s + ID_TOKEN_TTL_S,\n', '        "exp": now_s + 2 * ID_TOKEN_TTL_S,\n', ACT),
    ('mint signs without the check', O, '    may_mint(f.job, f.run, attempt, declared)?;\n', '', ACT),
    ('ActionsOidcKey switches with no wait', G, '        SecretName::ActionsOidcKey => Ok(&[Probe, Wait, Switch]),\n',
     '        SecretName::ActionsOidcKey => Ok(&[Probe, Switch]),\n', REL),
    ('ActionsOidcKey waits the MintWrapKey publish', OP, '                SecretName::ActionsOidcKey => dwell.oidc_published,\n',
     '                SecretName::ActionsOidcKey => dwell.rotate_published,\n', REL),
    ('the probe passes a slot holding the active key', L, '        if slots[old].as_str() == Some(kid) {\n',
     '        if false {\n', OPS),
    ('the probe passes sites with different keys', L, '            Some((s0, k0)) if k0 != kid => {\n',
     '            Some((s0, k0)) if false && k0 != kid => {\n', OPS),
]


def main():
    env = dict(os.environ, CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, 'target', 'plants')),
               CARGO_INCREMENTAL='0')
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, args = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        text = orig.decode()
        assert text.count(old) == 1, (name, 'anchor found', text.count(old))
        try:
            open(p, 'w').write(text.replace(old, new, 1))
            r = subprocess.run(['timeout', '-k', '10', '600', 'cargo', 'test', '-q', *args], cwd=ROOT, env=env,
                               capture_output=True, text=True)
            out = r.stdout + r.stderr
            if r.returncode == 124:
                verdict = 'CAUGHT (timed out)'
            elif r.returncode == 0:
                verdict = 'MISSED'
            elif 'error[E' in out or 'could not compile' in out:
                verdict = 'DID NOT COMPILE'
            else:
                verdict = 'CAUGHT'
            caught += verdict.startswith('CAUGHT')
        finally:
            open(p, 'wb').write(orig)
        assert open(p, 'rb').read() == orig
        print(f'{i} {name}: {verdict}', flush=True)
    print(f'{caught} of {len(pick)} caught')
    sys.exit(0 if caught == len(pick) else 1)


if __name__ == '__main__':
    main()
