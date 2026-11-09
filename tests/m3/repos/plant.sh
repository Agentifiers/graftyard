#!/bin/bash
# The m3-repos branch's planted bugs (rest:repos create, update and delete, repository format 32.13): each is applied
# alone to a clean tree, the gate tests run, and the plant is reverted. A plant passes the check only if at least one
# gate test fails on it. Run from the worktree root.
set -u
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0
# Each plant is undone with `git checkout`, which would also drop uncommitted work.
if [ -n "$(git status --porcelain -- crates)" ]; then
	echo "commit or stash changes under crates/ first" >&2
	exit 1
fi
# Prints each failing test's name and every binary's result line.
run() {
	cargo test -j 4 -q --no-fail-fast -p graftyard-git --lib --test issues --test transfer 2>&1 |
		grep -E "^    [a-z0-9_:]+$|test result" | sort -u
	cargo test -j 4 -q --no-fail-fast -p graftyard-api --test repos 2>&1 |
		grep -E "^    [a-z0-9_:]+$|test result" | sort -u
}
# `PLANTS_ONLY=a,b` runs only those plants.
plant() {
	local name=$1 file=$2 old=$3 new=$4
	if [ -n "${PLANTS_ONLY:-}" ] && [[ ",$PLANTS_ONLY," != *",$name,"* ]]; then return; fi
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

# A writer creates (initialises) a repository: `InitRepo` skips its Admin check.
plant init-without-admin crates/git/src/issue_ops.rs \
	'            IssueOp::InitRepo(s) => {
                need(RepoPermission::Admin)?;' \
	'            IssueOp::InitRepo(s) => {'
# A re-sent create overwrites the first settings and `created_at`: `InitRepo` is not once.
plant init-not-once crates/git/src/issue_ops.rs \
	'                if v.settings().is_some() || crate::tiers::created(v.core()).is_some() {' \
	'                if false && crate::tiers::created(v.core()).is_some() {'
# A create names another owner than the chain's.
plant init-any-owner crates/git/src/issue_ops.rs \
	'                if s.owner != owner || s.enterprise != authz.and_then(|a| a.enterprise()) {' \
	'                if false && s.owner != owner {'
# A second `Created` moves `created_at`.
plant created-overwritten crates/git/src/tiers.rs \
	'                if get_rec::<Timestamp>(w, "created", &[]).is_none() {' \
	'                if get_rec::<Timestamp>(w, "created", &[]).is_none() || true {'
# A writer deletes the repository: `DeleteRepo` skips its Admin check.
plant delete-without-admin crates/git/src/issue_ops.rs \
	'            IssueOp::DeleteRepo { retain_until } => {
                need(RepoPermission::Admin)?;' \
	'            IssueOp::DeleteRepo { retain_until } => {'
# A tombstone that is already due: `retain_until` is not checked against the entry.
plant delete-already-due crates/git/src/issue_ops.rs \
	'                if *retain_until <= v.at {' \
	'                if false && *retain_until <= v.at {'
# A deleted repository still takes issue and settings writes.
plant deleted-takes-writes crates/git/src/issue_ops.rs \
	'        if crate::tiers::deleted(v.core()).is_some() {
            return match &self.op {' \
	'        if false && crate::tiers::deleted(v.core()).is_some() {
            return match &self.op {'
# A deleted repository still takes pushes.
plant deleted-takes-pushes crates/git/src/repo.rs \
	'    if crate::tiers::deleted(s.core()).is_some() {
        return false;' \
	'    if false && crate::tiers::deleted(s.core()).is_some() {
        return false;'
# The deprecated `private` wins over `visibility`.
plant private-over-visibility crates/api/src/repos.rs \
	'    let vis = match body.get("visibility").and_then(Value::as_str) {' \
	'    let vis = match body.get("visibility").and_then(Value::as_str).filter(|_| body.get("private").is_none()) {'
# A user's repository may be internal.
plant user-internal crates/api/src/repos.rs \
	'        Some("internal") if org => Some(Visibility::Internal),' \
	'        Some("internal") => Some(Visibility::Internal),'
# Every merge method off is accepted.
plant no-merge-method crates/api/src/repos.rs \
	'    if s.merge_methods.is_empty() {
        let e = FieldError' \
	'    if false && s.merge_methods.is_empty() {
        let e = FieldError'
# `created_at` is the epoch, not the `Created` entry.
plant created-at-epoch crates/api/src/repos.rs \
	'    let at = graftyard_git::tiers::created(r.core).unwrap_or(Timestamp(0));' \
	'    let at = graftyard_git::tiers::created(r.core).map(|_| Timestamp(0)).unwrap_or(Timestamp(0));'
# The README commit's tree is packed as a blob.
plant readme-tree-as-blob crates/git/src/init.rs \
	'    entry(&mut pack, 2, &tree);' \
	'    entry(&mut pack, 3, &tree);'
# The README commit's tree names the README by the wrong id.
plant readme-wrong-blob-id crates/git/src/init.rs \
	'    let blob_id = object_id("blob", &blob);' \
	'    let blob_id = object_id("tree", &blob);'
