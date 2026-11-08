#!/bin/bash
# The m3-rows branch's planted bugs (BF-2, BF-31, BF-34, BF-35, BF-36): each is applied alone to a clean tree, the
# gate tests run, and the plant is reverted. A plant passes the check only if at least one gate test fails on it.
# Run from the worktree root.
set -u
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0
# Each plant is undone with `git checkout`, which would also drop uncommitted work.
if [ -n "$(git status --porcelain -- crates)" ]; then
	echo "commit or stash changes under crates/ first" >&2
	exit 1
fi
# Prints each failing test's name and every binary's result line.
run() {
	cargo test -j 4 -q --no-fail-fast -p graftyard-identity --lib --test m3 --test wp3 2>&1 |
		grep -E "^    [a-z0-9_:]+$|test result" | sort -u
	cargo test -j 4 -q --no-fail-fast -p graftyard-git --test push --test issues --test agent_gates 2>&1 |
		grep -E "^    [a-z0-9_]+$|test result" | sort -u
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

# BF-31: a `github_pat_` reaches every repository of its owner, not only the ones it names.
plant pat-reaches-every-repo crates/identity/src/access.rs \
	'            Some(RepoScope::Selected(r)) => r.contains(&repo.repo()),' \
	'            Some(RepoScope::Selected(_)) => true,'
# BF-31: a `github_pat_` with `contents: read` pushes (any granted level satisfies the need).
plant pat-read-writes crates/identity/src/access.rs \
	'needs.is_empty() || needs.iter().any(|(name, level)| granted(name) >= Some(*level)) {' \
	'needs.is_empty() || needs.iter().any(|(name, _)| granted(name).is_some()) {'
# BF-31: a `github_pat_` for another owner's repositories reaches this one.
plant pat-any-owner crates/identity/src/access.rs \
	'    let reaches = f.resource_owner.is_none_or(|o| o == repo.owner())' \
	'    let reaches = true'
# BF-2 and BF-31: a push does not check the token's scope at all (`Push` admits a stored token by its row only).
plant push-ignores-token-scope crates/git/src/repo.rs \
	'            &[("contents", graftyard_identity::Access::Write)],
            &access,
        ) == Decision::Allow' \
	'            &[("contents", graftyard_identity::Access::Write)],
            &access,
        ) != Decision::Hidden'
# BF-31: revoking a `ghu_` leaves its `ghr_` live (and the reverse).
plant pair-not-revoked crates/identity/src/tokens.rs \
	'            if let Some(other) = m.fields().get(hash).and_then(|f| f.paired) {' \
	'            if let Some(other) = None::<[u8; 32]>.filter(|_| m.fields().get(hash).is_some()) {'
# BF-35: `AddKey` lists text under a fingerprint it does not hash to.
plant key-text-unchecked crates/identity/src/identity.rs \
	'                if key_fingerprint(key) != Some(*fingerprint) {' \
	'                if key_fingerprint(key).is_none() {'
# BF-35: `AddKey` lists a key whose `Tokens` row never committed.
plant key-not-committed crates/identity/src/identity.rs \
	'                let credential = CredentialRef::SshKey(*fingerprint);
                if !live_for(tokens_dep(deps, &credential)?, &credential, account) {' \
	'                let credential = CredentialRef::SshKey(*fingerprint);
                if false && !live_for(tokens_dep(deps, &credential)?, &credential, account) {'
# BF-34: anyone with access withdraws an invitation.
plant cancel-by-anyone crates/git/src/issue_ops.rs \
	'            IssueOp::CancelInvitation { id } => {
                need(RepoPermission::Admin)?;' \
	'            IssueOp::CancelInvitation { id } => {'
# BF-34: an org member gets a collaborator invitation (the `Collaborator` write is not refused).
plant member-invited crates/git/src/issue_ops.rs \
	'                need(RepoPermission::Admin)?;
                if is_org_member(authz, owner, *user) {' \
	'                need(RepoPermission::Admin)?;
                if false && is_org_member(authz, owner, *user) {'
# BF-34: the leader decides issue commands at time 0, so a new repository invitation has already lapsed.
plant invitation-at-epoch crates/git/src/issue_ops/call.rs \
	'let view = RepoView::new(s.fold().tiers(), core, meta, s.at());' \
	'let view = RepoView::new(s.fold().tiers(), core, meta, graftyard_types::Timestamp(0));'
# BF-36: any member of the install org sets `InstanceAuth`.
plant instance-auth-by-member crates/identity/src/authz.rs \
	'                if !actor.is_some_and(|u| s.is_admin(u)) {
                    return Err(AuthzErr::Forbidden);
                }
                one(AuthzEvent::InstanceAuth(a.clone()), AuthzOut::Done)' \
	'                if actor.is_none() {
                    return Err(AuthzErr::Forbidden);
                }
                one(AuthzEvent::InstanceAuth(a.clone()), AuthzOut::Done)'
# BF-36: an id_token signed by another key is accepted.
plant oidc-signature-unchecked crates/identity/src/oidc.rs \
	'    if !verified {
        return Err(OidcError::Signature);' \
	'    if false && !verified {
        return Err(OidcError::Signature);'
# BF-36: an id_token for another client is accepted.
plant oidc-audience-unchecked crates/identity/src/oidc.rs \
	'    if !ours {' \
	'    if false && !ours {'
# BF-36: an id_token from another sign-in (a replayed one) is accepted.
plant oidc-nonce-unchecked crates/identity/src/oidc.rs \
	'    if c.nonce.as_deref() != Some(nonce) {' \
	'    if false && c.nonce.as_deref() != Some(nonce) {'
# BF-36: an id_token from another issuer is accepted.
plant oidc-issuer-unchecked crates/identity/src/oidc.rs \
	'    if c.iss != issuer {' \
	'    if false && c.iss != issuer {'
# BF-36: an expired id_token is accepted.
plant oidc-expiry-unchecked crates/identity/src/oidc.rs \
	'    if c.exp <= now_s {' \
	'    if false && c.exp <= now_s {'
