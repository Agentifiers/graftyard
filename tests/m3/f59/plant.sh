#!/bin/bash
# F59's planted bugs: each is applied alone to a clean tree, the gate tests run, and the plant is reverted. A plant
# passes the check only if at least one gate test (including its build) fails on it. Run from the worktree root.
set -u
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0
# Each plant is undone with `git checkout`, which would also drop uncommitted source work. Uncommitted tests are safe.
if [ -n "$(git status --porcelain -- crates/identity/src crates/git/src)" ]; then
	echo "commit or stash changes under crates/identity/src and crates/git/src first" >&2
	exit 1
fi

caught=0
missed=0
errors=0

run() {
	local rc=0
	cargo test -q -p graftyard-identity --test audit || rc=1
	cargo test -q -p graftyard-git --test lifecycle --test f59 --test issues || rc=1
	return "$rc"
}

# `PLANTS_ONLY=a,b` runs only those plants.
plant() {
	local name=$1 file=$2 old=$3 new=$4
	if [ -n "${PLANTS_ONLY:-}" ] && [[ ",$PLANTS_ONLY," != *",$name,"* ]]; then return; fi
	if ! python3 - "$file" "$old" "$new" <<'PY'
import sys
p, old, new = sys.argv[1:]
s = open(p).read()
assert s.count(old) == 1, f"{old!r} found {s.count(old)} times"
open(p, 'w').write(s.replace(old, new))
PY
	then
		echo "PLANT $name: anchor was not found exactly once" >&2
		errors=$((errors + 1))
		return
	fi
	echo "== $name"
	if run; then
		echo "PLANT $name: MISSED"
		missed=$((missed + 1))
	else
		echo "PLANT $name: CAUGHT"
		caught=$((caught + 1))
	fi
	git checkout -q -- "$file"
}

# A create commit point no longer owes its finisher anything.
plant audit-create-not-owed crates/identity/src/audit.rs \
	'            AuthzEvent::RepoCreate { key, repo, init } => {
                Some(Owed::Create { owner, key: key.clone(), repo: *repo, init: init.clone(), at: e.at })
            }' \
	'            AuthzEvent::RepoCreate { .. } => None,'
# A deleted name no longer owes its repository tombstone.
plant audit-delete-not-owed crates/identity/src/audit.rs \
	'            AuthzEvent::RepoName { entry: Some(RepoEntry::Deleted { repo, retain_until }), .. } => {
                Some(Owed::Delete { owner, repo: *repo, retain_until: *retain_until })
            }' \
	'            AuthzEvent::RepoName { entry: Some(RepoEntry::Deleted { .. }), .. } => None,'
# The finisher initializes a repository without first committing its RepoOwner route.
plant create-skips-repo-owner crates/identity/src/finish.rs \
	'        None => return Ok(Some(names(NameKey::Repo(c.repo), NamesOp::RepoOwner { repo: c.repo, owner: c.owner }))),' \
	'        None => {}'
# A just-committed create is immediately old enough to abort.
plant create-aborts-immediately crates/identity/src/finish.rs \
	'            if now.0 >= c.at.0.saturating_add(CREATE_ABORT_AFTER.as_millis() as u64) {' \
	'            if now.0 >= c.at.0.saturating_add(0) {'
# A create that never forms is retried forever instead of aborting.
plant create-never-aborts crates/identity/src/finish.rs \
	'            if now.0 >= c.at.0.saturating_add(CREATE_ABORT_AFTER.as_millis() as u64) {' \
	'            if false && now.0 >= c.at.0.saturating_add(CREATE_ABORT_AFTER.as_millis() as u64) {'
# The abort runs before life is inspected, so even a created repository can be aborted.
plant create-aborts-created crates/identity/src/finish.rs \
	'    match life.ok_or(())? {
        RepoLife::Created | RepoLife::Deleted => Ok(None),
        RepoLife::NoGroup | RepoLife::Open => {
            if now.0 >= c.at.0.saturating_add(CREATE_ABORT_AFTER.as_millis() as u64) {
                let op = AuthzOp::AbortCreate { key: key.clone(), repo: c.repo };
                return Ok(Some(Step::Authz(
                    ChainId::Authz(c.owner),
                    Box::new(AuthzCmd { cred: CredentialShard::NONE, op }),
                )));
            }
            let init = RepoStep::Init { owner: c.owner, enterprise: authz.enterprise(), init: c.init.into() };
            Ok(Some(Step::Repo(c.repo, init)))
        }
    }' \
	'    if now.0 >= c.at.0.saturating_add(CREATE_ABORT_AFTER.as_millis() as u64) {
        let op = AuthzOp::AbortCreate { key: key.clone(), repo: c.repo };
        return Ok(Some(Step::Authz(
            ChainId::Authz(c.owner),
            Box::new(AuthzCmd { cred: CredentialShard::NONE, op }),
        )));
    }
    match life.ok_or(())? {
        RepoLife::Created | RepoLife::Deleted => Ok(None),
        RepoLife::NoGroup | RepoLife::Open => {
            let init = RepoStep::Init { owner: c.owner, enterprise: authz.enterprise(), init: c.init.into() };
            Ok(Some(Step::Repo(c.repo, init)))
        }
    }'
# The finisher discards the owner's current enterprise instead of putting it in the first settings.
plant create-drops-enterprise crates/identity/src/finish.rs \
	'            let init = RepoStep::Init { owner: c.owner, enterprise: authz.enterprise(), init: c.init.into() };' \
	'            let init = RepoStep::Init { owner: c.owner, enterprise: None, init: c.init.into() };'
# A delete with a different retention from the name's tombstone is still sent.
plant delete-ignores-retain-until crates/identity/src/finish.rs \
	'    if authz.deleted(repo) != Some(retain_until) || now >= retain_until || repo_owner != Some(owner) {' \
	'    if now >= retain_until || repo_owner != Some(owner) {'
# Any caller, not just the Audit consumer's operator credential, can abort a create.
plant abort-create-without-operator crates/identity/src/authz.rs \
	'            AuthzOp::AbortCreate { key, repo } => {
                if !self.cred.is_operator() {
                    return Err(AuthzErr::Forbidden);
                }' \
	'            AuthzOp::AbortCreate { key, repo } => {'
# AbortCreate releases any name, even one that is not Live for this repository.
plant abort-create-without-live-name crates/identity/src/authz.rs \
	'                match s.name(key) {
                    Some(RepoEntry::Live { repo: r, .. }) if r == *repo => {
                        Ok((smallvec![AuthzEvent::RepoName { key: key.clone(), entry: None }], AuthzOut::Done))
                    }
                    _ => Ok((SmallVec::new(), AuthzOut::Done)),
                }' \
	'                Ok((smallvec![AuthzEvent::RepoName { key: key.clone(), entry: None }], AuthzOut::Done))'
# CreateRepoWith records the Live name but no RepoCreate event for Audit to find.
plant create-repo-without-event crates/identity/src/authz.rs \
	'                        AuthzEvent::RepoCreate { key, repo, init: init.clone() },
' \
	''
# CreateRepoWith accepts arbitrarily large recorded settings.
plant create-repo-without-init-cap crates/identity/src/authz.rs \
	'                if init.len() > CREATE_INIT_MAX {' \
	'                if false && init.len() > CREATE_INIT_MAX {'
# A follower accepts a RepoCreate event without the matching Live name.
plant follower-accepts-any-repo-create crates/identity/src/authz.rs \
	'            AuthzEvent::RepoCreate { key, repo, .. } if !matches!(self.name(key), Some(RepoEntry::Live { repo: r, .. }) if r == *repo) => {
                Err(format!("{key:?} is not live for {repo:?}").into())
            }' \
	'            AuthzEvent::RepoCreate { .. } => Ok(()),'
# Lifecycle::Init no longer requires a Live name.
plant lifecycle-init-without-live crates/git/src/lifecycle.rs \
	'                let live = authz.names.values().any(|e| matches!(e, RepoEntry::Live { repo: r, .. } if *r == repo));
                if !live {
                    return Err(LifecycleErr::NotLive);
                }
' \
	''
# A second Init overwrites the first settings and Created event.
plant lifecycle-init-not-once crates/git/src/lifecycle.rs \
	'                if created(s.core()).is_some() || get_rec::<RepoSettings>(s.core(), "settings", &[]).is_some() {' \
	'                if false && (created(s.core()).is_some() || get_rec::<RepoSettings>(s.core(), "settings", &[]).is_some()) {'
# Init accepts settings for another owner or enterprise.
plant lifecycle-init-without-owner-check crates/git/src/lifecycle.rs \
	'                if Some(settings.owner) != s.authz_owner() || settings.enterprise != authz.enterprise() {
                    return Err(LifecycleErr::WrongOwner);
                }
' \
	''
# Lifecycle::Delete no longer requires the matching Deleted name.
plant lifecycle-delete-without-name crates/git/src/lifecycle.rs \
	'                let held = authz.names.values().any(
                    |e| matches!(e, RepoEntry::Deleted { repo: r, retain_until: t } if *r == repo && t == retain_until),
                );
                if !held {
                    return Err(LifecycleErr::NotDeleted);
                }
' \
	''
# Delete accepts an already-due tombstone.
plant lifecycle-delete-already-due crates/git/src/lifecycle.rs \
	'                if *retain_until <= s.at() {' \
	'                if false && *retain_until <= s.at() {'
# The settings from the request keep their enterprise instead of the owner chain's current one.
plant init-settings-keeps-enterprise crates/git/src/lifecycle.rs \
	'    s.enterprise = enterprise;
' \
	''
# A delete finisher step is mapped with a retention one millisecond later than the name's.
plant of-step-changes-retain-until crates/git/src/lifecycle.rs \
	'            Some(Ok(Lifecycle { op: LifecycleOp::Delete { retain_until: *retain_until }, request }))' \
	'            Some(Ok(Lifecycle { op: LifecycleOp::Delete { retain_until: Timestamp(retain_until.0 + 1) }, request }))'
# The edge's late InitRepo can initialize a create after Audit aborted its name.
plant issue-init-without-live crates/git/src/issue_ops.rs \
	'                if !live {
                    return Err(IssueErr::NotFound);
                }' \
	'                if false && !live {
                    return Err(IssueErr::NotFound);
                }'

echo "TALLY: $caught CAUGHT, $missed MISSED, $errors ANCHOR ERRORS"
if [ "$missed" -ne 0 ] || [ "$errors" -ne 0 ]; then exit 1; fi
