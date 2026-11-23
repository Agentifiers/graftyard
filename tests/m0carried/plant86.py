#!/usr/bin/env python3
"""AM2-WP1, `graft-mcp` and the host's grant rules: planted bugs in LF-6's rules (`token/scoped`'s grant check, the
grant generation, the client credentials) and in journey A7's (PKCE, audience, consent, state, narrowing, the held
`ghu_`, CIMD). Plants one mutant at a time, runs the tests that guard it, and restores the file byte for byte. Each
line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant86.py          # every mutant
    python3 tests/m0carried/plant86.py 0 2      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
API = 'crates/api/src/mcp.rs'
ID = 'crates/identity/src/identity.rs'
FLOW = 'workers/mcp/src/flow.rs'
LIB = 'workers/mcp/src/lib.rs'
LF6 = ['cargo', 'test', '-q', '-p', 'graftyard-mcp', '--test', 'lf6']
A7 = ['cargo', 'test', '-q', '-p', 'graftyard-mcp', '--test', 'a7']
BOTH = ['cargo', 'test', '-q', '-p', 'graftyard-mcp']

MUTANTS = [
    # LF-6, the host's rules.
    ('token/scoped accepts a write above the grant', API,
     '    if write > g.write {\n', '    if false && write > g.write {\n', LF6),
    ("token/scoped accepts another user's grant", API,
     '    if grant.account != user {\n', '    if false && grant.account != user {\n', LF6),
    ('token/scoped from Graftyard MCP accepts a request with no grant', API,
     '        None => return Err(bad("graftyard_grant", "missing_field")),\n',
     '        None => GrantRef { account: AccountId(DbId(1)), grant: GrantId(DbId(1)) },\n', LF6),
    ('the grant endpoints take any client secret', API,
     'id == app.client.as_str() && graftyard_identity::appauth::client_secret_ok(&app.client_secrets, &secret)',
     'id == app.client.as_str() || graftyard_identity::appauth::client_secret_ok(&app.client_secrets, &secret)', LF6),
    ("a user token of another App acts on the user's grants", API,
     '                && f.and_then(|f| f.app) == Some(mcp.id) =>\n', '                =>\n', LF6),
    ('a stale generation moves the grant on instead of revoking it', ID,
     '                if *generation != r.grant.generation {\n', '                if false {\n', BOTH),
    ("the Identity leader records a scoped token above the grant's write", ID,
     '                if *write > r.grant.write {\n', '                if false {\n', LF6),
    ("revoking a grant leaves its tokens live", API,
     '        let revoke: Vec<[u8; 32]> = row.map(|r| r.tokens.into_keys().chain(r.upstream).collect()).unwrap_or_default();\n',
     '        let revoke: Vec<[u8; 32]> = row.map(|_| Vec::new()).unwrap_or_default();\n', BOTH),
    ("a scoped token the grant's leader refused stays live", API,
     '        revoke_all(h, &[tok.hash()]).await?;\n', '', LF6),
    # A7 and L0-08, graft-mcp's rules.
    ('the token endpoint skips PKCE', FLOW,
     '            if s256(get("code_verifier")) != c.challenge {\n', '            if false {\n', A7),
    ('a token with another audience is accepted', FLOW,
     '    if t.aud.0.as_ref() != format!("{}/", e.origin()) {\n', '    if false {\n', A7),
    ('a cross-site consent POST is accepted', FLOW,
     '    req.header("origin") == Some(e.origin()) && req.header("sec-fetch-site").is_none_or(|s| s == "same-origin")\n',
     '    req.header("origin").is_some() || req.headers.is_empty()\n', A7),
    ("the callback takes a state its cookie did not bind", FLOW,
     '    if flow.is_none() || q.get("state").map(String::as_str) != Some(upstate) || exp <= e.now() {\n',
     '    if flow.is_none() || exp <= e.now() {\n', A7),
    ('a refresh may widen its grant', FLOW,
     '                s => match Scope::parse(s).filter(|w| w.within(had)) {\n',
     '                s => match Scope::parse(s) {\n', A7),
    ('a refresh skips the generation, so a replay is not caught', FLOW,
     '            let generation = match spend_generation(e, &r.grant, r.generation).await {\n',
     '            let generation = match Ok::<u32, bool>(r.generation + 1) {\n', A7),
    ('the held ghu_ is never deleted after a refresh', FLOW,
     '            let _ = e.edge("DELETE", &format!("/api/v3{path}"), &h, Some(body)).await;\n',
     '            let _ = (h, path, body);\n', A7),
    ("a CIMD document for another client id is accepted", LIB,
     '    if doc.client_id != id {\n', '    if false {\n', A7),
    ('a CIMD document over 5 KB is accepted', LIB,
     '    if f.body.len() > CIMD_MAX_BYTES {\n', '    if false {\n', A7),
    ('a CIMD redirect is accepted', LIB,
     '    if f.status != 200 {\n', '    if f.status >= 400 {\n', A7),
    ('the resource metadata advertises every write scope', LIB,
     '"scopes_supported": ["write:read"], "resource_name"',
     '"scopes_supported": ["write:read", "write:write"], "resource_name"', A7),
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


if __name__ == '__main__':
    main()
