#!/usr/bin/env python3
"""M5-WP5's pure half: planted bugs in the change feed, the export endpoints, and export tokens
(`crates/git/src/export.rs`, `crates/git/src/key.rs`, `crates/identity/src/export_token.rs`; BF-49's parts that do not
depend on M5-WP4). Plants one mutant at a time, runs the tests that guard it, and restores the file byte for byte. Each
line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant83.py          # every mutant
    python3 tests/m0carried/plant83.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
EXP = 'crates/git/src/export.rs'
KEY = 'crates/git/src/key.rs'
TOK = 'crates/identity/src/export_token.rs'
# The integration test runs unfiltered: a name filter would also apply to it and skip BF-49.
GIT = ['sh', '-c', 'cargo test -q -p graftyard-git --test export && cargo test -q -p graftyard-git --lib -- export:: key::']
ID = ['cargo', 'test', '-q', '-p', 'graftyard-identity', '--lib', '--', 'export_token']

MUTANTS = [
    ("plan takes a chain at its cursor", EXP, '(*i > at).then_some((*c, at, *i))', '(*i >= at).then_some((*c, at, *i))', GIT),
    ("a full batch takes the chains nearest their cursor", EXP,
     'moved.sort_by(|a, b| (b.2.0 - b.1.0).cmp(&(a.2.0 - a.1.0)).then(a.0.cmp(&b.0)));',
     'moved.sort_by(|a, b| (a.2.0 - a.1.0).cmp(&(b.2.0 - b.1.0)).then(a.0.cmp(&b.0)));', GIT),
    ("a clock behind the newest object writes an earlier position", EXP,
     'Some(p) if p.minute >= minute => (p.minute, p.seq + 1),', 'Some(p) if p.minute == minute => (p.minute, p.seq + 1),', GIT),
    ("a cursor moves back", EXP, '*at = (*at).max(*i);', '*at = *i;', GIT),
    ("recovery folds in objects the state already holds", EXP,
     'if self.last.is_none_or(|l| b.position() > l) {', 'if true {', GIT),
    ("the state's position is not advanced", EXP, 'self.last = self.last.max(Some(batch.position()));', '', GIT),
    ("check_object accepts a gap after the cursor", EXP,
     'let starts = if from == 1 { first.0 >= 1 && first.0 <= to.0 } else { first.0 == from };',
     'let starts = if from == 1 { first.0 >= 1 && first.0 <= to.0 } else { first.0 >= from };', GIT),
    ("a frame's digest is not checked", EXP, 'if Sha256::digest(body).as_slice() != sum {', 'if false && Sha256::digest(body).as_slice() != sum {', GIT),
    ("decode ignores frames past the last chain", EXP, '''    if frames.next().is_some() {
        return Err("frames past the last chain's entries".into());
    }''', '', GIT),
    ("an export key's seq is not zero-padded", KEY, 'write!(f, "export/{minute:012}/{seq:010}")', 'write!(f, "export/{minute:012}/{seq}")', GIT),
    ("from=0 is accepted", EXP, '''                if from.0 == 0 {
                    return Err(ExportError::BadCursor("from 0: the first entry is 1".into()));
                }''', '', GIT),
    ("a limit over 1,000 is accepted", EXP, 'Ok(n) if (1..=ENTRIES_PAGE).contains(&n) => n,', 'Ok(n) if n >= 1 => n,', GIT),
    ("DELETE reads a batch page", EXP, 'let want = if matches!(route, Route::RevokeToken) { "DELETE" } else { "GET" };',
     'let want = if matches!(route, Route::RevokeToken) { "DELETE" } else { method };', GIT),
    ("an entries page ignores the byte cap", EXP,
     'if n > 0 && (n == limit || out.len() + 36 + row.len() > ENTRIES_PAGE_BYTES) {', 'if n > 0 && n == limit {', GIT),
    ("an entries page serves past the commit index", EXP, '''        if i > to {
            break;
        }''', '', GIT),
    ("entries below the first stored index are served", EXP, 'if from < first && !archived {', 'if false && from < first && !archived {', GIT),
    ("an entries page serves a gap", EXP, '''        if i != want {
            return Err(ExportError::Gone(format!("entry {} is missing from the stored log", want.0)));
        }''', '', GIT),
    ("an out-of-scope chain's entries are served", EXP, '(_, Some(c)) if !covers(c) => Err(ExportError::NotFound),',
     '(_, Some(c)) if false && !covers(c) => Err(ExportError::NotFound),', GIT),
    ("a pack outside the scope is served", EXP,
     'Route::Pack { network, .. } => Some(ChainId::Network(*network)),', 'Route::Pack { .. } => None,', GIT),
    ("a batches cursor past retention is served", EXP,
     '(after.minute * 60_000).saturating_add(EXPORT_RETENTION_MS) < now_ms', 'false', GIT),
    ("the batches page has no Link next", EXP, 'objects.len() >= BATCHES_PAGE || objects.iter()', 'objects.len() > BATCHES_PAGE || objects.iter()', GIT),
    ("the snapshot is the newest at all, not at or below", EXP, '&& *to <= at', '', GIT),
    ("a token's signature is not checked", TOK, '''    vk.verify_strict(head.as_bytes(), &ed25519_dalek::Signature::from_bytes(&sig))
        .map_err(|_| ExportTokenError::Signature)?;''', '    let _ = (vk, sig);', ID),
    ("a token for another install verifies", TOK, '''    if claims.install != install {
        return Err(ExportTokenError::Install);
    }''', '', ID),
    ("an expired token verifies", TOK, 'if claims.exp <= now {', 'if claims.exp < now {', ID),
    ("a token may live past 400 days", TOK, 'if claims.exp.0.saturating_sub(claims.iat.0) > EXPORT_TOKEN_MAX.as_millis() as u64 {',
     'if false {', ID),
    ("an org scope covers system chains", TOK, 'scope == Scope::Install || graftyard_release::view::covers(scope, chain, owner)',
     'scope == Scope::Install || !matches!(scope, Scope::Repo(_)) || graftyard_release::view::covers(scope, chain, owner)', ID),
    ("Install scope leaves out Release", TOK, 'scope == Scope::Install || graftyard_release::view::covers(scope, chain, owner)',
     'graftyard_release::view::covers(scope, chain, owner)', ID),
    ("revocation is under no credential", TOK,
     'cred: CredentialShard { account: None, key: Some(CredentialKey::Jti { shard: jti_shard(&jti), jti: jti.0 }) },',
     'cred: CredentialShard { account: None, key: None },', ID),
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
