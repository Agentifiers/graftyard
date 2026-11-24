#!/usr/bin/env python3
"""M5-WP5's vault Worker and `graftyard vault init`: planted bugs in the vault's pass (`workers/vault/src/lib.rs`) and
the CLI's lock rules, token, and release check (`cli/src/vault.rs`, `crates/release/src/ops.rs` `check_vault`;
`decisions/DECISIONS.md`, `vault-worker` and `vault-release`). Plants one mutant at a
time, runs the tests that guard it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT
COMPILE. Run from the repository root:

    python3 tests/m0carried/plant88.py          # every mutant
    python3 tests/m0carried/plant88.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
LIB = 'workers/vault/src/lib.rs'
CLI = 'cli/src/vault.rs'
REPO = 'crates/git/src/repo.rs'  # `stored_packs`, which the vault and the lifeboat share
OPS = 'crates/release/src/ops.rs'
VAULT = ['cargo', 'test', '-q', '-p', 'graftyard-vault']
INIT = ['cargo', 'test', '-q', '-p', 'graftyard', '--', 'vault']
RELEASE = ['sh', '-c', 'cargo test -q -p graftyard-release -- the_vault_worker && cargo test -q -p graftyard -- vault']

MUTANTS = [
    # The feed.
    ("a batch at or before the vault's last is stored again", LIB,
     '            if st.feed.last.is_some_and(|l| pos <= l) {', '            if st.feed.last.is_some_and(|l| pos < l) {',
     VAULT),
    ("an object is stored without the cursor check", LIB,
     '            export::check_object(&o, &cursors).map_err(|e| format!("feed {}: {e}", pos.text()))?;\n', '', VAULT),
    ("the state is written without its etag", LIB,
     '            match h.put_state(&st.encode(), etag.as_deref()).await? {',
     '            match h.put_state(&st.encode(), None).await? {', VAULT),
    ("a 410 on batches is an error, not a resync", LIB,
     '            Got::Status(410) if after.is_some() && !resync => {',
     '            Got::Status(410) if false => {', VAULT),
    ("a resync leaves the gap before the oldest object unfilled", LIB,
     '                        st.backfill.insert(*c, Backfill { next: Index(at.0 + 1), until: *first });\n', '', VAULT),
    # Backfill.
    ("a chain first carried past 1 owes nothing before it", LIB,
     '            st.backfill.insert(*chain, Backfill { next: Index(1), until: *first });', '', VAULT),
    ("an archived backfill takes no snapshot", LIB,
     '                st.copies.insert(Copy::Snapshot { chain, at: Index(b.until.0 - 1) });', '', VAULT),
    # What entries name.
    ("a promotion's pack is not copied", REPO,
     '            RepoEvent::Promoted(p) => all.push(p.pack.clone()),', '            RepoEvent::Promoted(_) => {}', VAULT),
    ("an Archived snapshot is not copied", LIB,
     '                        st.copies.insert(Copy::Snapshot { chain, at: to });', '', VAULT),
    ("a consumed pack is a GC candidate at once", LIB,
     '                            after: Timestamp(e.at.0 + CONSUMED_KEEP),', '                            after: e.at,',
     VAULT),
    # Copies.
    ("a corrupt copy stays owed forever", LIB,
     '            Copied::Status(404 | 410) | Copied::Corrupt(_) => {',
     '            Copied::Corrupt(_) => continue,\n            Copied::Status(404 | 410) => {', VAULT),
    ("a pack check takes a short body", LIB,
     '        if self.seen != self.bytes {', '        if self.seen > self.bytes {', VAULT),
    ("a pack check skips the digest", LIB,
     '        if got != self.want {', '        if false {', VAULT),
    ("a snapshot object's digest is not checked", LIB,
     "                out.push(SnapPart::End { ok: sha.finalize().as_slice() == b });",
     "                out.push(SnapPart::End { ok: { let _ = sha; true } });", VAULT),
    # GC.
    ("a pack is deleted at the first 404", LIB,
     '        Some(gone) if now.0 >= gone.0.saturating_add(restore_window_ms()) => GcStep::Delete,',
     '        Some(_) => GcStep::Delete,', VAULT),
    ("a 410 counts as GC", LIB,
     '        match (h.pack_status(network, id).await?, step) {',
     '        match (match h.pack_status(network, id).await? { 410 => 404, s => s }, step) {', VAULT),
    ("a later 200 does not reset a candidate's clock", LIB,
     '                st.gc.insert(key, Gc { probed: Some(now), gone: None, ..g });',
     '                st.gc.insert(key, Gc { probed: Some(now), ..g });', VAULT),
    ("a delete does not probe the install again", LIB,
     '            (404, GcStep::Delete) => {', '            (_, GcStep::Delete) => {', VAULT),
    # A refused token.
    ("a 401 on a copy is counted missed, not stopped", LIB,
     '            Copied::Status(401) => return Err(refused(&format!("{c:?}"), 401)),\n', '', VAULT),
    # Create-only puts under a lock (R2 checks the lock before `If-None-Match`).
    ("a refused create-only put fails the pass", LIB,
     '        Ok(false) => None,\n        Err(e) => Some(e),', '        Ok(false) => None::<String>,\n        Err(e) => return Err(e),', VAULT),
    ("a refused put is taken as stored without looking", LIB,
     '        Taken::Different(why) => Err(format!("{key} holds another object: {why}")),',
     '        Taken::Different(_) => Ok(false),', VAULT),
    ("a different digest at the key passes as the same", LIB,
     '        (Some(want), Some(got)) if got == want.hex() => Taken::Same,', '        (Some(_), Some(_)) => Taken::Same,', VAULT),
    ("a different size at the key passes as the same", LIB,
     '    if h.size != len {', '    if false {', VAULT),
    # The signed vault artifact.
    ("check_vault skips the signature", OPS,
     '    verify_raw(release_key, record, &sig).map_err(|_| bad("release.json does not verify under Keys.release"))?;\n    let rec: serde_json::Value = serde_json::from_slice(record).map_err(|_| bad("release.json is not JSON"))?;\n    let listed: BTreeMap<&str, &str> = rec["files"]\n        .as_array()\n        .into_iter()\n        .flatten()\n        .filter_map(|f| Some((f["name"].as_str()?, f["sha256"].as_str()?)))\n        .filter(',
     '    let _ = (release_key, sig);\n    let rec: serde_json::Value = serde_json::from_slice(record).map_err(|_| bad("release.json is not JSON"))?;\n    let listed: BTreeMap<&str, &str> = rec["files"]\n        .as_array()\n        .into_iter()\n        .flatten()\n        .filter_map(|f| Some((f["name"].as_str()?, f["sha256"].as_str()?)))\n        .filter(', RELEASE),
    ("check_vault takes a vault/ file the record does not list", OPS,
     '        let Some(module) = name.strip_prefix(VAULT_FOLDER) else { continue };\n        let want = crate::types::hex(&Sha256::digest(bytes));\n        if listed.get(name.as_str()) != Some(&want.as_str()) {',
     '        let Some(module) = name.strip_prefix(VAULT_FOLDER) else { continue };\n        let want = crate::types::hex(&Sha256::digest(bytes));\n        if listed.get(name.as_str()).is_some_and(|l| *l != want.as_str()) {', RELEASE),
    ("check_vault takes a release missing a listed module", OPS,
     '        return Err(bad(format!("the release holds no {missing}, which release.json lists")));', '        let _ = missing;', RELEASE),
    ("upgrade verifies after its first API call", CLI,
     '    let artifact = verified(&a.release)?;\n    let release = hex(&artifact.release.0);\n    let (versions, _) = cf.deployed(a.script)?;',
     '    let (versions, _) = cf.deployed(a.script)?;\n    let artifact = verified(&a.release)?;\n    let release = hex(&artifact.release.0);', INIT),
    ("upgrade resets the bindings", CLI,
     '        "keep_bindings": ["r2_bucket", "plain_text", "secret_text"],', '        "keep_bindings": [],', INIT),
    ("upgrade uploads again a release already at 100%", CLI,
     '    if serving(cf, a.script)?.as_deref() == Some(release.as_str()) {',
     '    if false && serving(cf, a.script)?.as_deref() == Some(release.as_str()) {', INIT),
    # `vault init`.
    ("the lock covers 30 days, not 35", CLI,
     '"condition": { "type": "Age", "maxAgeSeconds": LOCK_DAYS * 86_400 },',
     '"condition": { "type": "Age", "maxAgeSeconds": 30 * 86_400 },', INIT),
    ("a rerun drops the account's other lock rules", CLI,
     'cf.bucket_lock(a.bucket)?.into_iter().filter(|rule| !ours.iter().any(|our| our["id"] == rule["id"])).collect();',
     'Vec::new();', INIT),
    ("init signs with a key that is not the install's", CLI,
     '    if customer != anchor.customer {\n        return Err(', '    if false {\n        return Err(', INIT),
    ("init's token is org-scoped", CLI,
     '    let c = claims(anchor.install, Scope::Install, "vault", a.days, at, Ulid::new(at, random()))?;',
     '    let c = claims(anchor.install, Scope::Owner(graftyard_types::AccountId(graftyard_types::DbId(1))), "vault", '
     'a.days, at, Ulid::new(at, random()))?;', INIT),
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
