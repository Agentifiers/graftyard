#!/bin/bash
# BF-30's planted bugs: each is applied alone to a clean tree, the gate tests run, and the plant is reverted. A plant
# counts as caught only if at least one gate test fails on it. Run from the worktree root:
#   bash tests/m3/finish/plant.sh <out file>
# As an orb service, which restarts a command that exits, it writes `DONE` and waits for its stop.
set -u
out=${1:?usage: plant.sh <out file>}
exec >"$out" 2>&1
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0
if [ -n "$(git status --porcelain -- crates)" ]; then
	echo "commit or stash changes under crates/ first" >&2
	exit 1
fi
run() {
	cargo test -j 4 -q --no-fail-fast -p graftyard-identity --test audit --test group_move --test wp3 2>&1 |
		grep -E "^    [a-z0-9_]+$|test result" | sort -u
	cargo test -j 4 -q --no-fail-fast -p graftyard-git --test transfer --test push 2>&1 |
		grep -E "^    [a-z0-9_]+$|test result" | sort -u
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

# The transfer Settings skips `deps`: it records the new owner without reading that owner's Authz chain, so a
# Settings no reservation backs moves the repository.
plant settings-skips-deps crates/git/src/transfer.rs \
	'            .ok_or(TransferErr::NotRead)?;
        let repo = s.fold().repo.ok_or(TransferErr::NotReserved)?;
        if !names_repo(authz, repo) {
            return Err(TransferErr::NotReserved);
        }
        if graftyard_identity::AuthzRead::enterprise(authz) != self.enterprise {
            return Err(TransferErr::WrongEnterprise);
        }' \
	'            .ok_or(TransferErr::NotRead)?;
        let _ = authz;
        let repo = s.fold().repo.ok_or(TransferErr::NotReserved)?;'
# The transfer Settings declares no dependency: the leader reads nothing, and decide fails closed or, if it trusted
# the command, moves the owner unchecked.
plant settings-declares-no-deps crates/git/src/transfer.rs \
	'        smallvec![ChainId::Authz(self.owner)]' \
	'        SmallVec::new()'
# Audit never scans an Authz chain: a Reserved name owes nothing, so a transfer whose finisher died stays unfinished.
plant audit-skips-authz-cursor crates/identity/src/audit.rs \
	'            AuthzEvent::RepoName { key, entry: Some(RepoEntry::Reserved { repo, from }) } => {
                Some(Owed::Transfer { repo: *repo, from: *from, to: owner, key: key.clone() })
            }' \
	'            AuthzEvent::RepoName { entry: Some(RepoEntry::Reserved { .. }), .. } => None,'
# Audit's Authz cursor command is a no-op, so every batch starts again from the chain's first entry.
plant authz-cursor-never-moves crates/identity/src/authz.rs \
	'            return Ok((smallvec![AuthzEvent::AuditCursor { through: *through }], AuthzOut::Done));' \
	'            return Ok((SmallVec::new(), AuthzOut::Done));'
# The move drops the leader's site first: no leadership transfer before the swap that removes it.
plant move-drops-leader-first crates/identity/src/group_move.rs \
	'    if leaving(leader)
        && let Some(stay) = staying(&vs)
    {
        return MoveStep::Transfer { to: stay };
    }' \
	''
# The Joint's old half leaves while the leader is the site it drops: no transfer to a voter of the new half first.
plant leave-without-transfer crates/identity/src/group_move.rs \
	'            if !vs.contains(&leader) {' \
	'            if false {'
# The finisher records Settings before the group moves, so the new owner's route names a placement the group is not in.
plant settings-before-move crates/identity/src/finish.rs \
	'    if !same_place(v.from_placement, v.to_placement) && !group_at(v.group, v.to_placement) {
        return Some(TransferStep::Move { to: v.to_placement.clone() });
    }
    if v.settings_owner != t.to || v.settings_enterprise != v.to_authz.enterprise() {
        return Some(TransferStep::Settings { owner: t.to, enterprise: v.to_authz.enterprise() });
    }' \
	'    if v.settings_owner != t.to || v.settings_enterprise != v.to_authz.enterprise() {
        return Some(TransferStep::Settings { owner: t.to, enterprise: v.to_authz.enterprise() });
    }
    if !same_place(v.from_placement, v.to_placement) && !group_at(v.group, v.to_placement) {
        return Some(TransferStep::Move { to: v.to_placement.clone() });
    }'
# The finisher compares only the owner: an org changing enterprise never gets Settings with the new enterprise.
plant settings-ignores-enterprise crates/identity/src/finish.rs \
	'        if owner == org && enterprise != want {' \
	'        if owner != org && enterprise != want {'
# The id route finds only a Live name, as before hosted run a: between the old owner's Alias and RepoOwner, the
# repository's `/repositories/{id}` had no name and answered 404.
plant id-name-live-only crates/identity/src/authz.rs \
	'        find(true).or_else(|| find(false))' \
	'        find(true)'
echo DONE
sleep infinity
