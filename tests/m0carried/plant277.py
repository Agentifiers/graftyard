#!/usr/bin/env python3
"""AM3-WP2a, base forks in their network (`build/BUILD-PLAN.md`; `decisions/DECISIONS.md`, "`fork-network-read`").
Head 1's mutants: a fork's network on its chain (`crates/git/src/tiers.rs`, `Genesis` and the `fork` row), in the
access rows (`crates/api/src/hosted.rs`), learned by the edge (`workers/edge/src/lib.rs`, `nets`) and carried by a
shared gate entry (`workers/edge/src/colo.rs`, `gate_body`), and on `Names`' `RepoOwner` row for the id-only routes
(`crates/identity/src/names.rs`, Names 0x10.4). The review fixes after head 1: the network fixed on the request by its own
read (`Repo::fix`) and held by its gate entry, the clone front's key built again on it, a Genesis naming another network
refused, and the format order (`decisions/DECISIONS.md`, "`format-order`"). Later heads append their mutants here. Run from the repository root:

    python3 tests/m0carried/plant277.py          # every mutant
    python3 tests/m0carried/plant277.py 0 2      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

TIERS = 'crates/git/src/tiers.rs'
HOSTED = 'crates/api/src/hosted.rs'
EDGE = 'workers/edge/src/lib.rs'
COLO = 'workers/edge/src/colo.rs'
NAMES = 'crates/identity/src/names.rs'
CHAIN = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'agent_load', 'a_fork_chain_names_its_network']
ACCESS = ['cargo', 'test', '-q', '-p', 'graftyard-api', '--test', 'surfaces', 'access_rows_include_a_forks']
ENTRY = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', 'a_gate_entry_carries_a_forks_network']
FIXED = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', 'a_forks_network_is_fixed_by_its_own_read']
FRONT = ['cargo', 'test', '-q', '-p', 'graftyard-edge', '--lib', 'a_clone_key_built_before_the_gate']
REPO = 'crates/git/src/repo.rs'
CORPUS = 'crates/git/tests/fold_corpus.rs'
ORDER = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'fold_corpus', 'a_new_events_format_is_above']
OWNER = ['cargo', 'test', '-q', '-p', 'graftyard-identity', '--test', 'm3', 'a_forks_repo_owner_row_carries']
MUTANTS = [
    ("a fork's Genesis writes no fork row", TIERS,
     '                if network.root() != self.repo {\n', '                if false {\n', CHAIN),
    ("a root's Genesis writes a fork row, moving its rows", TIERS,
     '                if network.root() != self.repo {\n', '                if true {\n', CHAIN),
    ("the access rows leave out the fork row (option A's edge, with no RepoOwner read)", HOSTED,
     '        c.extend(core.get("fork", &[]).map(|cols| ("fork".to_owned(), Vec::new(), cols)));\n', '', ACCESS),
    ("the edge keys a fork by its own id", EDGE,
     '        fork(repo).unwrap_or(NetworkId::of_root(repo))\n', '        NetworkId::of_root(repo)\n', FIXED),
    ("the edge stores a root's own id as a fork", EDGE,
     '        if network.root() == repo {\n            return;\n        }\n', '', FIXED),
    ("a root's shared gate entry carries a net, changing main's bytes", COLO,
     '        None => serde_json::json!({ "at": at.0 }).to_string(),\n',
     '        None => serde_json::json!({ "at": at.0, "net": null }).to_string(),\n', ENTRY),
    ("a shared gate entry drops a fork's network", COLO,
     '    Some((Timestamp(v["at"].as_u64()?), serde_json::from_value(v["net"].clone()).ok().flatten()))\n',
     '    Some((Timestamp(v["at"].as_u64()?), None))\n', ENTRY),
    ("a root's RepoOwnerIn writes ForkOwner, moving its Names row", NAMES,
     '                if network.root() == *repo {\n                    one(NamesEvent::RepoOwner',
     '                if false {\n                    one(NamesEvent::RepoOwner', OWNER),
    ("a follower stores a ForkOwner whose network is its own id", NAMES,
     '            NamesEvent::ForkOwner { repo, network, .. } if network.root() == *repo => {',
     '            NamesEvent::ForkOwner { repo, network, .. } if network.root() == *repo && false => {', OWNER),
    ("a fork's network is not kept in the Names fold", NAMES,
     '            t.more.networks().put(*repo, *network);\n', '', OWNER),
    # The review fixes (AM2, after head 1).
    ("a request reads the isolate's map every time, so an overflow moves its packs under the fork's own id", EDGE,
     '        self.net.get().unwrap_or_else(|| nets::get(self.id))\n', '        nets::get(self.id)\n', FIXED),
    # `GateIo::hold` and `share` pass `Repo::fork()` through; they need a live `Ctx`, so the entry's own storage is
    # what a native test reads back.
    ("a gate entry drops its fill's network, so a later share puts a bare {at} for a fork", EDGE,
     '                c.insert(k, Entry { at, d, foreign, rows, fork });\n',
     '                c.insert(k, Entry { at, d, foreign, rows, fork: None });\n', FIXED),
    ("a root fixes a network of its own id as a fork, so its entry grows a net", EDGE,
     '        self.net.get().filter(|n| n.root() != self.id)\n', '        self.net.get()\n', FIXED),
    ("the clone front's key on the guessed network is kept, so a stale entry is served", EDGE,
     '        Some(k) if k.network != network => (Some(CloneKey { network, ..k }), false),\n',
     '        Some(k) if false => (Some(CloneKey { network, ..k }), false),\n', FRONT),
    ("a second Genesis may name another network", REPO,
     '                    Some(f) if f.network != *network => {\n', '                    Some(f) if false && f.network != *network => {\n',
     CHAIN),
    ("Genesis back at 32.31, below TaskResumed's 32.33", CORPUS, '        50 => 34,\n', '        50 => 31,\n', ORDER),
]

if __name__ == '__main__':
    run(MUTANTS)
