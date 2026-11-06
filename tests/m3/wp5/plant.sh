#!/bin/bash
# M3-WP5's planted bugs: each is applied alone to a clean tree, the gate tests run, and the plant is reverted. A plant
# passes the check only if at least one gate test fails on it. Run from the worktree root.
set -u
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0
# Each plant is undone with `git checkout`, which would also drop uncommitted work.
if [ -n "$(git status --porcelain -- crates)" ]; then
	echo "commit or stash changes under crates/ first" >&2
	exit 1
fi
run() {
	cargo test -j 3 -q -p graftyard-types --lib placement 2>&1 | grep -E "^test .*FAILED|test result"
	cargo test -j 3 -q -p graftyard-fold --test install 2>&1 | grep -E "^test .*FAILED|test result"
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

# BF-28: a learner's object name without its kind letter, so learner slot k and voter slot k on one site collide.
plant name-without-kind crates/types/src/placement.rs \
	'format!("{chain}#{generation}#{}{slot}", kind.letter())' \
	'format!("{chain}#{generation}#f{slot}")'
# BF-28: a learner addressed by its site, as voters are, so it shares its voter's progress and quorum count.
plant learner-addressed-by-site crates/types/src/placement.rs \
	'MemberKind::Learner => learner_addr(self.slot),' \
	'MemberKind::Learner => self.site,'
# BF-28: a quorum counted over every ack, not over the config's voters, so learners make a commit.
plant learner-acks-count crates/raft/src/quorum.rs \
	'let got = p.voters().iter().filter(|m| Some(m.site) != lost && yes.contains(&m.site)).count();' \
	'let got = yes.iter().filter(|s| Some(**s) != lost).count();'
# BF-28: a learner campaigns.
plant learner-campaigns crates/raft/src/member/election.rs \
	'        if self.is_learner() {
            return Verdict::MayNotLead;
        }' \
	''
# BF-29: the leader never sends InstallSnapshot, so a follower behind the boundary is stuck on catchup.
plant no-install crates/raft/src/member/install.rs \
	'if prev >= source.through {' \
	'if prev >= source.through || source.through.0 > 0 {'
# BF-29: the installing member acks before it has installed.
plant ack-before-install crates/raft/src/member/install.rs \
	'        self.out.install.push(InstallWanted { from, install: req });' \
	'        let res = self.install_res(&req, true, req.source.through);
        self.send(from, Message::AppendRes(res));
        self.out.install.push(InstallWanted { from, install: req });'
# BF-29: the host skips the archives and installs at the snapshot's index.
plant skip-archives crates/fold/tests/install.rs \
	'        for e in entries.iter().filter(|e| e.index > to) {' \
	'        for e in entries.iter().filter(|e| e.index > to).take(0) {'
# BF-29: the installed base names the wrong entry, so the next append does not chain.
plant wrong-base crates/raft/src/member/install.rs \
	'let base = LogBase { index: s.through, term: s.through_term, digest: s.through_digest };' \
	'let base = LogBase { index: s.through, term: s.through_term, digest: crate::PREV_ZERO };'
# F48: refold_segment skips the snapshot's rows.
plant refold-skips-rows crates/fold/src/snapshot.rs \
	'                store.put(def.name, r.key.clone(), Stored { written: r.written, cols: r.cols.clone() });' \
	'                let _ = (def, r);'
