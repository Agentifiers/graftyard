#!/bin/bash
# M3-WP4's planted bugs: each is applied alone to a clean tree, the gate tests run, and the plant is reverted. A plant
# passes the check only if at least one gate test fails on it. Run from the worktree root.
set -u
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$PWD/target/plants}
# Each plant is undone with `git checkout`, which would also drop uncommitted work.
if [ -n "$(git status --porcelain -- crates)" ]; then
	echo "commit or stash changes under crates/ first" >&2
	exit 1
fi
# Prints each failing test's name and every binary's result line; a run the timeout ended counts as caught
# (README, "Planted bugs").
run() {
	timeout -k 10 600 cargo test -j 4 -q --no-fail-fast -p graftyard-identity --lib --test wp4 --test m3 --test wp3 2>&1 |
		grep -E "^    [a-z0-9_:]+$|test result" | sort -u
	[ "${PIPESTATUS[0]}" = 124 ] && echo "    (timed out)"
	timeout -k 10 600 cargo test -j 4 -q --no-fail-fast -p graftyard-git --test push 2>&1 |
		grep -E "^    [a-z0-9_]+$|test result" | sort -u
	[ "${PIPESTATUS[0]}" = 124 ] && echo "    (timed out)"
}
plant() {
	local name=$1 file=$2 old=$3 new=$4
	python3 - "$file" "$old" "$new" <<'PY' || { echo "PLANT $name: pattern not found"; return; }
import sys
p, old, new = sys.argv[1:]
s = open(p).read()
assert s.count(old) == 1, f"{old!r} found {s.count(old)} times"
open(p, 'w').write(s.replace(old, new))
PY
	echo "== $name"
	run
	git checkout -q -- "$file"
}

# BF-27: the leader does not read the `jti` row, so a revoked installation token keeps writing.
plant jti-unchecked crates/identity/src/credential.rs \
	'if on_tokens(shard, own, deps, |r| r.jti_revoked(&jti, graftyard_types::Timestamp(0)))? {' \
	'if on_tokens(shard, own, deps, |_| false)? {'
# An installation token names no account, so a check of `account.is_none()` alone makes it the operator.
plant token-is-operator crates/identity/src/authz.rs \
	'            None => self.cred.is_operator(),' \
	'            None => true,'
# `DELETE /installation/token` from anyone: the leader skips the "own token" check.
plant revoke-anyones-jti crates/identity/src/tokens.rs \
	'if !self.operator() && !own {' \
	'if !self.operator() && own {'
# The edge ignores a retired key, so a stolen old key signs forever.
plant retired-key-honored crates/identity/src/stateless.rs \
	'    if row.retired.is_some_and(|r| r <= now) {' \
	'    if row.retired.is_some_and(|r| r < Timestamp(0)) {'
# `graft-ops` caches the unwrapped key forever.
plant key-cache-never-lapses crates/identity/src/stateless.rs \
	'now.0.saturating_sub(at.0) < ms(SIGNING_KEY_CACHE))' \
	'now.0.saturating_sub(at.0) < u64::MAX)'
# Rotation signs with the new key the moment it is published, before every edge can verify it.
plant rotation-signs-early crates/identity/src/stateless.rs \
	'None if now.0 >= new.published.0 + ms(INSTALLATION_TOKEN_TTL) => RotationStep::SignWithNew,' \
	'None if now.0 >= new.published.0 => RotationStep::SignWithNew,'
# The token request grants a permission the App lacks if the installation has it.
plant narrow-ignores-app crates/identity/src/stateless.rs \
	'if !ok(app) || !ok(installation) {' \
	'if !ok(installation) {'
# The token request names a repository outside the installation's selection.
plant narrow-outside-selection crates/identity/src/stateless.rs \
	'            Some(r) if inside(r) => ids.push(r),' \
	'            Some(r) => ids.push(r),'
# An App JWT whose signature fails is accepted.
plant app-jwt-signature-unchecked crates/identity/src/appauth.rs \
	'    if !verified {' \
	'    if false && !verified {'
# A wrong client secret passes.
plant wrong-secret-accepted crates/identity/src/appauth.rs \
	'secrets.iter().fold(false, |ok, s| ok | (s.0 == h.0))' \
	'secrets.iter().fold(false, |ok, s| ok | (s.0 == h.0)) || !secret.is_empty()'
# An installation wider than its App's permissions commits.
plant install-beyond-app crates/identity/src/authz.rs \
	'        if !row.permissions.within(&record.permissions) {' \
	'        if false && !row.permissions.within(&record.permissions) {'
# An installation commits before its `Names` route, its commit point.
plant install-before-route crates/identity/src/authz.rs \
	'        if names.installation(*id) != Some((row.app, owner)) {' \
	'        if false && names.installation(*id) != Some((row.app, owner)) {'
# Uninstall does not fail tokens minted before it: `authorize` skips `not_before`.
plant not-before-unchecked crates/identity/src/access.rs \
	'    if row.app != c.app || c.iat < state.not_before {' \
	'    if row.app != c.app {'
# `authorize` honors the claims alone, not the installation's current selection.
plant claims-not-intersected crates/identity/src/access.rs \
	'    (reaches(&row.repos) && reaches(&c.repos)).then_some(row)' \
	'    reaches(&c.repos).then_some(row)'
