#!/bin/bash
# M3 close's planted bugs: each is applied alone to a clean tree, the gate tests run, and the plant is reverted. A
# plant counts as caught only if at least one gate test fails on it. Run from the worktree root.
set -u
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0
if [ -n "$(git status --porcelain -- crates)" ]; then
	echo "commit or stash changes under crates/ first" >&2
	exit 1
fi
run() {
	cargo test -j 4 -q --no-fail-fast -p graftyard-identity --test audit --test wp3 --test m3 2>&1 |
		grep -E "^    [a-z0-9_]+$|test result" | sort -u
	cargo test -j 4 -q --no-fail-fast -p graftyard-git --test issues 2>&1 |
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

# Audit: the cursor moves before the work it covers, so a batch lost after the cursor leaves a credential unlisted.
plant cursor-before-work crates/identity/src/audit.rs \
	'        while let Some(o) = self.owed.get(self.at) {' \
	'        if !self.cursor_sent && let Some(step) = cursor(self.chain, self.through) {
            return Want::Submit(step);
        }
        while let Some(o) = self.owed.get(self.at) {'
# Audit: no minute's wait, so the consumer races the edge's own follow-up.
plant no-minute crates/identity/src/audit.rs \
	'        if e.at.0.saturating_add(after) > now.0 {' \
	'        if e.at.0 > now.0 {'
# Audit: an SSH key's entry owes nothing, so a key whose edge died stays unlisted.
plant ssh-key-not-owed crates/identity/src/audit.rs \
	'            TokensEvent::SshKey { fingerprint, account, .. } => {
                Some(Owed::Credential { account: *account, credential: CredentialRef::SshKey(*fingerprint) })
            }' \
	''
# Audit: a user may move the cursor, skipping the consumer past work.
plant cursor-by-anyone crates/identity/src/tokens.rs \
	'            TokensOp::AuditCursor { through, .. } => {
                if !self.operator() {
                    return Err(TokensErr::Forbidden);
                }' \
	'            TokensOp::AuditCursor { through, .. } => {'
# Audit: the cursor can move back, so a new leader reads entries again forever.
plant cursor-moves-back crates/identity/src/tokens.rs \
	'            self.more.more = self.more.more.max(*through);' \
	'            self.more.more = *through;'
# Audit: every cursor entry is followed by another, so an idle shard commits forever.
plant cursor-chases-itself crates/identity/src/audit.rs \
	'    through > from && (!owed.is_empty() || through.0 - from.0 >= CURSOR_STRIDE)' \
	'    through > from'
# Item 1: a ghs_ issue write is allowed whatever the App's permissions.
plant app-permission-ignored crates/identity/src/access.rs \
	'    let granted = |name: &str| c.permissions.0.get(name).min(row.permissions.0.get(name)).copied();' \
	'    let granted = |name: &str| c.permissions.0.get(name).max(row.permissions.0.get(name)).copied();'
# F51: a status takes the edge's minted id again, so a later status from a slow-clocked edge loses.
plant status-id-from-edge-clock crates/git/src/issue_ops.rs \
	'                let id = top.map_or(st.id.0.0, |t| st.id.0.0.max(t + 1));' \
	'                let id = top.map_or(st.id.0.0, |t| if t == st.id.0.0 { t + 1 } else { st.id.0.0 });'
