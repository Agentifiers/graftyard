#!/bin/bash
# M3-WP3's planted bugs: each is applied alone to a clean tree, the gate tests run, and the plant is reverted. A plant
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
	timeout -k 10 600 cargo test -j 4 -q --no-fail-fast -p graftyard-identity --test wp3 --test m3 --test access 2>&1 |
		grep -E "^    [a-z0-9_]+$|test result" | sort -u
	[ "${PIPESTATUS[0]}" = 124 ] && echo "    (timed out)"
	timeout -k 10 600 cargo test -j 4 -q --no-fail-fast -p graftyard-git --test push --test issues 2>&1 |
		grep -E "^    [a-z0-9_]+$|test result" | sort -u
	[ "${PIPESTATUS[0]}" = 124 ] && echo "    (timed out)"
}
plant() {
	local name=$1 file=$2 old=$3 new=$4
	python3 - "$file" "$old" "$new" <<'EOF' || { echo "PLANT $name: pattern not found"; return; }
import sys
p, old, new = sys.argv[1:]
s = open(p).read()
assert s.count(old) == 1, f"{old!r} found {s.count(old)} times"
open(p, 'w').write(s.replace(old, new))
EOF
	echo "== $name"
	run
	git checkout -q -- "$file"
}

# BF-3: the leader skips the Account row's suspension, so a suspended user's PAT still pushes.
plant suspension-unchecked crates/identity/src/credential.rs \
	'            Some(r) if r.suspended => return Err(CredentialRefusal::Suspended(a)),' \
	''
# BF-3: one SSH key added from two edges gets two owners: the leader no longer refuses a taken fingerprint.
plant ssh-key-two-owners crates/identity/src/tokens.rs \
	'                if let Some(r) = s.ssh_key(fingerprint) {
                    return Err(TokensErr::KeyTaken { by: r.account });
                }' \
	''
# Sign out everywhere: a session issued before `sessions_not_before` still writes.
plant session-not-before-unchecked crates/identity/src/credential.rs \
	'            Some(r) if issued < r.sessions_not_before => return Err(CredentialRefusal::SignedOut),' \
	''
# BF-30: a push's deps keep the fold's first owner, so after a transfer's `Settings` they name a stale owner.
plant stale-owner-in-deps crates/git/src/repo.rs \
	'        d.extend(authz_chains(fold.authz_owner(), fold.authz_enterprise()));' \
	'        d.extend(authz_chains(fold.owner, fold.authz_enterprise()));'
# BF-30: the transfer's names go live before the group moved, so a route through the new owner misses the group.
plant live-before-move crates/identity/src/finish.rs \
	'    if !same_place(v.from_placement, v.to_placement) && !group_at(v.group, v.to_placement) {' \
	'    if false {'
# BF-33: a used code is spent again, so one code yields two tokens.
plant code-redeemable-twice crates/identity/src/tokens.rs \
	'                if c.used {
                    return Err(TokensErr::CodeUsed);
                }
                match &c.kind {' \
	'                match &c.kind {'
# BF-34: accepting an org invitation records `Accepted` without the membership.
plant accept-without-member crates/identity/src/authz.rs \
	'                    AuthzEvent::Member { user, role: Some(role) },' \
	''
# BF-34: a `Collaborator` invitation for an org member is not refused, so the member has both kinds of grant.
plant collaborator-for-org-member crates/git/src/issue_ops.rs \
	'                if is_org_member(authz, owner, *user) {
                    return Err(IssueErr::OrgMember);
                }
                if collaborator(v.core(), *user).is_some() {' \
	'                if collaborator(v.core(), *user).is_some() {'
# BF-34: an invitation never lapses.
plant invitation-never-lapses crates/identity/src/authz.rs \
	'        self.invitations.get(&id).filter(|r| r.expires > now)' \
	'        self.invitations.get(&id).filter(|_| now.0 > 0)'
# BF-34: `authorize` ignores `Collaborator` rows, so an outside collaborator loses access.
plant collaborators-ignored crates/identity/src/access.rs \
	'    let collab = repo.collaborator(user);' \
	'    let collab = repo.collaborator(user).filter(|_| false);'
# BF-35: the `Identity` side lists a credential with no live `Tokens` row.
plant credential-without-tokens-row crates/identity/src/identity.rs \
	'                if !live_for(tokens_dep(deps, credential)?, credential, account) {
                    return Err(IdentityErr::NotCommitted);
                }' \
	''
# BF-36: the operator, or any account, writes `InstanceAuth`.
plant instance-auth-by-anyone crates/identity/src/authz.rs \
	'                if !actor.is_some_and(|u| s.is_admin(u)) {
                    return Err(AuthzErr::Forbidden);
                }
                one(AuthzEvent::InstanceAuth' \
	'                one(AuthzEvent::InstanceAuth'
# BF-36: a suspended account starts a session.
plant sign-in-while-suspended crates/identity/src/identity.rs \
	'                if !n.account(account).is_some_and(|r| !r.suspended) {' \
	'                if n.account(account).is_none() {'
